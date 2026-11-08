//! Offline comparison of paired recorded continuity responses.
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kamimusuhi_runtime::continuity_eval::{ComparisonInput, EvaluationStatus, compare};

fn aliases_input(input: &Path, output: &Path) -> bool {
    if input == output
        || std::fs::canonicalize(output)
            .ok()
            .zip(std::fs::canonicalize(input).ok())
            .is_some_and(|(a, b)| a == b)
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(input), std::fs::metadata(output)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut output = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" if input.is_none() => {
                input = Some(PathBuf::from(args.next().ok_or("--input needs a path")?));
            }
            "--out" if output.is_none() => {
                output = Some(PathBuf::from(args.next().ok_or("--out needs a path")?));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: continuity-eval --input <paired.json> [--out <report.json>]\nOffline only. Exit 0: pass; 1: failed checks; 2: invalid input; 3: unverified human axes."
                );
                return Ok(ExitCode::SUCCESS);
            }
            _ => return Err("unknown or repeated argument".to_owned()),
        }
    }
    let input = input.ok_or("--input is required")?;
    if output
        .as_ref()
        .is_some_and(|path| aliases_input(&input, path))
    {
        return Err("output must not overwrite the input".to_owned());
    }
    let metadata = std::fs::metadata(&input).map_err(|_| "cannot inspect input")?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err("input must be a file of at most 16 MiB".to_owned());
    }
    let bytes = std::fs::read(input).map_err(|_| "cannot read input")?;
    let input: ComparisonInput =
        serde_json::from_slice(&bytes).map_err(|_| "invalid comparison JSON")?;
    let report = compare(&input).map_err(|error| error.to_string())?;
    let body = serde_json::to_string_pretty(&report).map_err(|_| "cannot serialize report")?;
    match output {
        Some(path) => {
            std::fs::write(path, format!("{body}\n")).map_err(|_| "cannot write report")?
        }
        None => println!("{body}"),
    }
    Ok(match report.comparison_status {
        EvaluationStatus::Pass => ExitCode::SUCCESS,
        EvaluationStatus::Fail => ExitCode::from(1),
        EvaluationStatus::Unverified => ExitCode::from(3),
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
