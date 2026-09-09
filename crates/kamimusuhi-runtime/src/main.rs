//! Kamimusuhi runtime binary.
//!
//! ```text
//! kamimusuhi-runtime init            --dir <path> [--resource <impl>] [--seed <n>]
//! kamimusuhi-runtime inspect         --dir <path> [--seed <n>]
//! kamimusuhi-runtime demo-continuity --dir <path> --phase first|resume
//!                                    [--resource <impl>] [--seed <n>]
//! ```
//!
//! Reports go to stdout as JSON so a harness can read them; a phase run by one
//! process reports to the outside world this way rather than by handing
//! anything to the next process.
//!
//! `--seed` fixes the ID sequence and the clock so the fixture is reproducible
//! from a clean checkout. Two processes sharing a runtime directory must use
//! different seeds or they would mint colliding IDs.

use std::process::ExitCode;

use kamimusuhi_runtime::config::GENERAL_SLOT;
use kamimusuhi_runtime::{
    DemoPhase, FakeImplementation, Runtime, RuntimeError, RuntimeOptions, inspect, scenario,
};

const USAGE: &str = "\
kamimusuhi-runtime <command> [options]

commands:
  init              prepare a runtime directory; creates an individual only
                    when the canonical database holds none
  inspect           read-only report of identity, lineage, memory, Library
                    and resource calls
  demo-continuity   run one phase of the deterministic restart scenario

options:
  --dir <path>      runtime directory (required)
  --phase <p>       demo-continuity only: first | resume
  --resource <i>    fake filling the general slot: fake-a | fake-b |
                    fake-unavailable
  --seed <n>        deterministic ID/clock seed
";

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("kamimusuhi-runtime: {error}");
            // 2 is reserved for usage so a harness can tell "you asked wrong"
            // from "the runtime refused".
            match error {
                RuntimeError::Usage(_) => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run() -> Result<String, RuntimeError> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or_else(|| RuntimeError::Usage(format!("no command given\n\n{USAGE}")))?;
    let options = Options::parse(args)?;

    match command.as_str() {
        "init" => {
            let runtime = Runtime::init(
                options.dir()?,
                options.runtime_options(),
                options.resource.unwrap_or(FakeImplementation::FakeA),
            )?;
            let report = inspect::inspect(&runtime)?;
            runtime.stopping();
            encode(&report)
        }
        "inspect" => {
            // Opened without claiming a writer epoch: inspection is a read.
            let runtime = Runtime::open(options.dir()?, options.runtime_options())?;
            let report = inspect::inspect(&runtime)?;
            runtime.stopping();
            encode(&report)
        }
        "demo-continuity" => {
            let phase = options.phase.ok_or_else(|| {
                RuntimeError::Usage("demo-continuity requires --phase first|resume".to_owned())
            })?;
            let mut runtime = Runtime::open(options.dir()?, options.runtime_options())?;
            // Replacing the resource is a config rewrite and nothing else: no
            // canonical write, no new individual, no lineage event.
            if let Some(resource) = options.resource {
                let mut config = runtime.config().clone();
                config.set_implementation(GENERAL_SLOT, resource);
                runtime.save_config(config)?;
            }
            let report = scenario::run(&mut runtime, phase)?;
            runtime.stopping();
            encode(&report)
        }
        "--help" | "-h" | "help" => Ok(USAGE.to_owned()),
        other => Err(RuntimeError::Usage(format!(
            "unknown command {other:?}\n\n{USAGE}"
        ))),
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, RuntimeError> {
    serde_json::to_string_pretty(value)
        .map_err(|source| RuntimeError::Usage(format!("could not encode report: {source}")))
}

#[derive(Debug, Default)]
struct Options {
    dir: Option<String>,
    phase: Option<DemoPhase>,
    resource: Option<FakeImplementation>,
    seed: Option<u64>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, RuntimeError> {
        let mut options = Self::default();
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next().ok_or_else(|| {
                    RuntimeError::Usage(format!("{flag} requires a value\n\n{USAGE}"))
                })
            };
            match flag.as_str() {
                "--dir" => options.dir = Some(value()?),
                "--phase" => options.phase = Some(value()?.parse()?),
                "--resource" => {
                    let raw = value()?;
                    options.resource = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!(
                            "unknown --resource {raw:?}; expected fake-a, fake-b or fake-unavailable"
                        ))
                    })?);
                }
                "--seed" => {
                    let raw = value()?;
                    options.seed = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!("--seed {raw:?} is not a number"))
                    })?);
                }
                other => {
                    return Err(RuntimeError::Usage(format!(
                        "unknown option {other:?}\n\n{USAGE}"
                    )));
                }
            }
        }
        Ok(options)
    }

    fn dir(&self) -> Result<&str, RuntimeError> {
        self.dir
            .as_deref()
            .ok_or_else(|| RuntimeError::Usage(format!("--dir is required\n\n{USAGE}")))
    }

    fn runtime_options(&self) -> RuntimeOptions {
        RuntimeOptions { seed: self.seed }
    }
}
