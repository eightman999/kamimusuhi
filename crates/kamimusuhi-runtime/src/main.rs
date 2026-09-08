//! Kamimusuhi runtime binary.
//!
//! `init`, `inspect` and `demo-continuity` are added in Wave 4. Until then the
//! binary only reports that no command is available so that the workspace
//! builds end to end.

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!(
        "kamimusuhi-runtime: no commands are implemented yet (planned: init, inspect, demo-continuity)"
    );
    ExitCode::from(2)
}
