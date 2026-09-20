mod app;
mod worker;

use std::path::PathBuf;

use kamimusuhi_core::routing::PrivacyConstraint;

fn main() {
    if let Err(error) = run() {
        eprintln!("kamimusuhi-desktop: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = DesktopOptions::parse(std::env::args().skip(1))?;
    let (commands, events) = worker::spawn(worker::WorkerConfig {
        runtime_dir: options.runtime_dir,
        subject: options.subject,
        privacy: options.privacy,
    });
    app::run(commands, events)
}

#[derive(Debug)]
struct DesktopOptions {
    runtime_dir: PathBuf,
    subject: String,
    privacy: PrivacyConstraint,
}

impl DesktopOptions {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut runtime_dir = PathBuf::from(".local/desktop");
        let mut subject = "local-user".to_owned();
        let mut privacy = PrivacyConstraint::LocalOnly;
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{flag} requires a value"))
            };
            match flag.as_str() {
                "--dir" => runtime_dir = PathBuf::from(value()?),
                "--subject" => {
                    subject = value()?;
                    if subject.is_empty()
                        || subject.len() > 80
                        || !subject.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                        })
                    {
                        return Err(
                            "--subject must be 1-80 ASCII letters, digits, '.', '_' or '-'"
                                .to_owned(),
                        );
                    }
                }
                "--privacy" => {
                    let raw = value()?;
                    privacy = raw.replace('-', "_").parse().map_err(|_| {
                        "--privacy must be local-only, no-external-service or unconstrained"
                            .to_owned()
                    })?;
                }
                "--help" | "-h" => {
                    println!(
                        "kamimusuhi-desktop [--dir <path>] [--subject <id>] [--privacy <scope>]\n\nNative dialogue GUI. The default privacy scope is local-only; use --privacy unconstrained explicitly for Jev/Grokbot external calls."
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown option {other:?}")),
            }
        }
        Ok(Self {
            runtime_dir,
            subject,
            privacy,
        })
    }
}
