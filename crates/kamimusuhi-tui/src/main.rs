//! `kami` — terminal front-end for the always-on Kamimusuhi individual (澪).
//! Talks to the resident HTTP API; the terminal UI never blocks on the network.

mod agents;
mod app;
mod ui;

use std::process::ExitCode;

use kamimusuhi_resident::remote::{self, RemoteConfig};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("kami: {message}");
            ExitCode::from(1)
        }
    }
}

struct Options {
    url: Option<String>,
    subject: String,
    view: Option<String>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut url = None;
        let mut subject = std::env::var("USER").unwrap_or_else(|_| "local-user".to_owned());
        let mut view = None;
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{flag} requires a value"))
            };
            match flag.as_str() {
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
                }
                "--help" | "-h" => {
                    println!(
                        "kami [--url <resident-url>] [--subject <id>] [--view chat|tasks|approvals|status|tools|agents]\n\nTerminal dialogue with the always-on Kamimusuhi individual (澪).\nResident URL: --url, $KAMIMUSUHI_URL or ~/.config/kamimusuhi/nodes.\nToken: $KAMIMUSUHI_NODE_TOKEN or ~/.config/kamimusuhi/node_token.\n\nKeys: Tab/Shift+Tab or F1-F6 switch views, Enter sends, Alt+Enter or\nCtrl+N inserts a newline, Ctrl+C quits. Per-view keys are in the footer."
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown option {other:?}")),
            }
        }
        if let Some(v) = &view
            && !matches!(
                v.as_str(),
                "chat" | "tasks" | "approvals" | "status" | "tools" | "agents"
            )
        {
            return Err(
                "--view must be chat, tasks, approvals, status, tools or agents".to_owned(),
            );
        }
        Ok(Self { url, subject, view })
    }
}

fn run() -> Result<(), String> {
    let options = Options::parse(std::env::args().skip(1))?;
    let (commands, events) = remote::spawn(RemoteConfig {
        url: options.url,
        subject: options.subject.clone(),
    });
    app::run(commands, events, options.subject, options.view)
}
