mod app;
mod remote;
mod remote_app;
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
    if !options.local {
        // Default: talk to the always-on individual through its resident.
        let subject = options
            .subject_explicit
            .clone()
            .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "local-user".to_owned()));
        let (commands, events) = remote::spawn(remote::RemoteConfig {
            url: options.url.clone(),
            subject: subject.clone(),
        });
        return remote_app::run(commands, events, subject, options.view.as_deref());
    }
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
    subject_explicit: Option<String>,
    privacy: PrivacyConstraint,
    /// `--local`: run the Jev test surface against a local runtime directory.
    local: bool,
    /// Resident URL for the default (resident) mode.
    url: Option<String>,
    /// Initial view in resident mode: chat | tasks | approvals | status | tools.
    view: Option<String>,
}

impl DesktopOptions {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut runtime_dir = PathBuf::from(".local/desktop");
        let mut subject = "local-user".to_owned();
        let mut privacy = PrivacyConstraint::LocalOnly;
        let mut subject_explicit = None;
        let mut local = false;
        let mut url = None;
        let mut view = None;
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{flag} requires a value"))
            };
            match flag.as_str() {
                "--dir" => {
                    runtime_dir = PathBuf::from(value()?);
                    local = true;
                }
                "--local" => local = true,
                "--url" => url = Some(value()?),
                "--view" => view = Some(value()?),
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
                    subject_explicit = Some(subject.clone());
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
                        "kamimusuhi-desktop [--url <resident-url>] [--subject <id>] [--view chat|tasks|approvals|status|tools]\n       kamimusuhi-desktop --local [--dir <path>] [--subject <id>] [--privacy <scope>]\n\nDefault: dialogue with the always-on individual through its resident (LAN, then Tailscale; token from ~/.config/kamimusuhi/node_token).\n--local: the Jev test surface against a local runtime. Its default privacy scope is local-only; use --privacy unconstrained explicitly for Jev/Grokbot external calls."
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown option {other:?}")),
            }
        }
        if privacy != PrivacyConstraint::LocalOnly {
            local = true;
        }
        Ok(Self {
            runtime_dir,
            subject,
            subject_explicit,
            privacy,
            local,
            url,
            view,
        })
    }
}
