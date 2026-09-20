//! Standalone, model-free K-CORE runner. No implicit init or backend activation.
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;
use std::time::Instant;

use kamimusuhi_runtime::RuntimeOptions;
use kamimusuhi_runtime::kcore::{KCore, KCoreConfig, KCoreError, MAX_FRAME_BYTES, PerceptFrame};

const USAGE: &str = "\
k-core --dir <initialized-runtime> [--config <json>] [--ticks <n>]
       [--interval-ms <1..60000>] [--stdin]

No model, GPU, API key or organ is required. No model is started automatically.
--ticks 0 (default) runs continuously; Ctrl-C exits without canonical mutation.
--stdin reads bounded numeric PerceptFrame JSONL; EOF drains the queue and stops.
Without --stdin, idle ticks never call a backend. Each output line is JSON.
Initialize separately with: kamimusuhi-runtime init --dir <path>
";

type Incoming = Result<(PerceptFrame, Instant), String>;

#[derive(Default)]
struct Options {
    dir: Option<PathBuf>,
    config: Option<PathBuf>,
    ticks: u64,
    interval_ms: Option<u64>,
    stdin: bool,
}

fn usage(message: impl Into<String>) -> KCoreError {
    KCoreError::Config(message.into())
}

fn parse() -> Result<Option<Options>, KCoreError> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            println!("{USAGE}");
            return Ok(None);
        }
        if flag == "--stdin" {
            options.stdin = true;
            continue;
        }
        if !matches!(
            flag.as_str(),
            "--dir" | "--config" | "--ticks" | "--interval-ms"
        ) {
            return Err(usage(format!("unknown option {flag}")));
        }
        let value = args
            .next()
            .ok_or_else(|| usage(format!("missing value for {flag}")))?;
        match flag.as_str() {
            "--dir" => options.dir = Some(value.into()),
            "--config" => options.config = Some(value.into()),
            "--ticks" => options.ticks = value.parse().map_err(|_| usage("invalid ticks"))?,
            "--interval-ms" => {
                options.interval_ms = Some(value.parse().map_err(|_| usage("invalid interval"))?);
            }
            _ => unreachable!(),
        }
    }
    if options.dir.is_none() {
        return Err(usage(format!("--dir is required\n{USAGE}")));
    }
    Ok(Some(options))
}

fn read_input(sender: SyncSender<Incoming>) {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        let mut bytes = Vec::new();
        let result = (&mut input)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes);
        let received_at = Instant::now();
        match result {
            Ok(0) => return,
            Err(_) => {
                let _ = sender.send(Err("stdin read failed".to_owned()));
                return;
            }
            Ok(_) if bytes.len() > MAX_FRAME_BYTES => {
                let _ = sender.send(Err("frame exceeds MAX_FRAME_BYTES".to_owned()));
                return;
            }
            Ok(_) => {}
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<PerceptFrame>(&bytes) {
            Ok(frame) => {
                if sender.send(Ok((frame, received_at))).is_err() {
                    return;
                }
            }
            Err(_) => {
                // Do not print untrusted input or keep parsing a broken stream.
                let _ = sender.send(Err("invalid PerceptFrame JSON".to_owned()));
                return;
            }
        }
    }
}

fn emit(output: &mut impl Write, value: &serde_json::Value) -> Result<(), KCoreError> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn run() -> Result<(), KCoreError> {
    let Some(options) = parse()? else {
        return Ok(());
    };
    let mut config = match &options.config {
        Some(path) => KCoreConfig::load(path)?,
        None => KCoreConfig::default(),
    };
    if let Some(interval_ms) = options.interval_ms {
        config.interval_ms = interval_ms;
    }
    config.validate()?;
    let capacity = config.queue_capacity;
    let mut core = KCore::open(
        options.dir.as_ref().expect("validated --dir"),
        RuntimeOptions::default(),
        config,
    )?;
    let receiver: Option<Receiver<Incoming>> = if options.stdin {
        let (sender, receiver) = sync_channel(capacity);
        thread::spawn(move || read_input(sender));
        Some(receiver)
    } else {
        None
    };
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    emit(
        &mut output,
        &serde_json::json!({
            "event": "kcore_boot", "canonical": core.inspect()?, "organs": core.descriptors(),
        }),
    )?;
    let mut completed = 0_u64;
    let mut eof = false;
    loop {
        let started = Instant::now();
        if let Some(receiver) = &receiver {
            // Backpressure: never drain more than the loop can retain.
            for _ in 0..capacity.saturating_sub(core.queued()) {
                match receiver.try_recv() {
                    Ok(Ok((mut frame, received_at))) => {
                        // Include time spent blocked in the reader/channel, not
                        // just time spent in the K-CORE queue.
                        let transit =
                            u64::try_from(received_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                        frame.age_ms = frame.age_ms.saturating_add(transit);
                        if let Err(reason) = core.ingest(frame) {
                            emit(
                                &mut output,
                                &serde_json::json!({
                                    "event": "ingress_rejected", "reason": reason,
                                }),
                            )?;
                        }
                    }
                    Ok(Err(error)) => return Err(usage(error)),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        eof = true;
                        break;
                    }
                }
            }
        }
        if eof && core.queued() == 0 {
            break;
        }
        let report = core.tick()?;
        completed = report.tick;
        emit(
            &mut output,
            &serde_json::json!({"event": "tick", "report": report}),
        )?;
        if options.ticks != 0 && completed >= options.ticks {
            break;
        }
        // No catch-up burst after a slow organ or blocked output sink.
        thread::sleep(core.interval().saturating_sub(started.elapsed()));
    }
    emit(
        &mut output,
        &serde_json::json!({
            "event": "kcore_stopped", "ticks": completed, "canonical": core.inspect()?,
        }),
    )?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("k-core: {error}");
            ExitCode::FAILURE
        }
    }
}
