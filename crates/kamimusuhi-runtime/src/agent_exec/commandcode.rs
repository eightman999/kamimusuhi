//! Command Code CLI preset, model-list parser and headless NDJSON events.
//!
//! Plan credits are `included_credit`, not free: even a model named
//! `*-free` was observed failing with "insufficient credits" on an account
//! without credits, so nothing here infers billing from a model name.
//! The Command Code Provider API (raw language model) would be a chat
//! provider, not an executor, and is out of scope here.

use serde_json::Value;

use super::catalog::RawModel;
use super::config::{
    DiscoverFormat, DiscoverSpec, OutputFormat, PermissionArgs, Preset, RunSpec, strings,
};
use super::output::ParsedRun;
use super::types::{ModelCapabilities, TaskEvent, TaskKind, TaskUsage};

pub fn preset() -> Preset {
    Preset {
        command: "cmd",
        discover: Some(DiscoverSpec {
            args: strings(&["--list-models"]),
            format: DiscoverFormat::CommandCodeText,
        }),
        run: Some(RunSpec {
            prefix: Vec::new(),
            // Workspaces are operator-allowlisted, so trusting one skips a
            // prompt headless mode could not answer.
            args: strings(&[
                "--output-format",
                "json",
                "--trust",
                "--skip-onboarding",
                "--no-auto-update",
                "-p",
                "{prompt}",
            ]),
            model_args: strings(&["--model", "{model}"]),
            resume_args: strings(&["--session", "{session}"]),
            output: OutputFormat::CommandCodeNdjson,
        }),
        permission_args: PermissionArgs {
            read_only: Some(strings(&["--permission-mode", "plan"])),
            workspace_write: Some(strings(&["--permission-mode", "auto-accept"])),
            autonomous_workspace: Some(strings(&["--yolo"])),
        },
        prefer_for: vec![TaskKind::Coding, TaskKind::Debug],
        acp: None,
        server: None,
    }
}

fn is_model_id(token: &str) -> bool {
    token
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && token
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_./".contains(&b))
}

/// `cmd --list-models`: `<id>   <description>` rows under vendor headings.
pub fn parse_models(text: &str) -> Result<Vec<RawModel>, String> {
    let models: Vec<RawModel> = text
        .lines()
        .filter_map(|line| {
            let (id, description) = line.split_once("  ")?;
            let id = id.trim();
            if !is_model_id(id) || line.starts_with(char::is_whitespace) {
                return None;
            }
            let description = description.trim();
            let lower = format!("{id} {description}").to_lowercase();
            Some(RawModel {
                label: (!description.is_empty()).then(|| description.to_owned()),
                capabilities: Some(ModelCapabilities {
                    coding: true,
                    reasoning: lower.contains("reason") || lower.contains("thinking"),
                    vision: lower.contains("vision") || lower.contains("multimodal"),
                    tools: true,
                    long_context: lower.contains("1m context") || lower.contains("long-context"),
                    fast: super::catalog::sounds_fast(&lower),
                }),
                ..RawModel::new(id.to_owned())
            })
        })
        .collect();
    if models.is_empty() {
        return Err("cmd --list-models: no model rows".to_owned());
    }
    Ok(models)
}

fn tool_name(event: &Value) -> String {
    ["name", "toolName", "tool"]
        .iter()
        .find_map(|k| event[*k].as_str())
        .or_else(|| event["tool"]["name"].as_str())
        .unwrap_or("tool")
        .to_owned()
}

fn usage(value: &Value) -> TaskUsage {
    TaskUsage {
        input_tokens: value["inputTokens"].as_u64(),
        output_tokens: value["outputTokens"].as_u64(),
        cache_read_tokens: value["cacheReadTokens"].as_u64(),
        cache_write_tokens: value["cacheWriteTokens"].as_u64(),
    }
}

/// One line of `cmd -p --output-format json`.
pub fn feed(run: &mut ParsedRun, line: &str) -> Vec<TaskEvent> {
    let Ok(line) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    match line["type"].as_str().unwrap_or("") {
        "event" => {
            let event = &line["event"];
            let kind = event["type"].as_str().unwrap_or("");
            match kind {
                "run_start" => {
                    if let Some(id) = event["sessionId"].as_str() {
                        run.session_id = Some(id.to_owned());
                        events.push(TaskEvent::Session { id: id.to_owned() });
                    }
                }
                "run_error" => {
                    run.error = event["error"]["message"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| Some(event["error"].to_string()));
                }
                _ if kind.starts_with("tool") => {
                    let finished = kind.ends_with("finished")
                        || kind.ends_with("end")
                        || kind.ends_with("result")
                        || kind.ends_with("complete");
                    let name = tool_name(event);
                    if finished {
                        run.tools.push(name.clone());
                    }
                    events.push(TaskEvent::Tool { name, finished });
                }
                "turn_start" => {
                    if let Some(n) = event["turnNumber"].as_u64() {
                        events.push(TaskEvent::Note {
                            text: format!("turn {n}"),
                        });
                    }
                }
                _ => {}
            }
        }
        "result" => {
            run.reported_success = Some(line["subtype"].as_str() == Some("success"));
            if let Some(text) = line["finalText"].as_str().filter(|t| !t.trim().is_empty()) {
                run.texts.push(text.to_owned());
            }
            if let Some(id) = line["sessionId"].as_str() {
                run.session_id = Some(id.to_owned());
            }
            run.usage = usage(&line["usage"]);
            if let Some(error) = line["error"].as_str() {
                run.error = Some(error.to_owned());
            }
        }
        _ => {}
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "Available models  ·  60 models

Open Source

deepseek/deepseek-v4-flash             fast hybrid-attention reasoning (default)
moonshotai/kimi-k2.5                   multimodal frontend coding
poolside/laguna-s-2.1-free             FREE open-weight agentic coding and long-horizon work

Anthropic

claude-sonnet-5                        best combo of speed & intelligence (recommended)

Pass the full id, or just the short name after the last \"/\":
cmd --model moonshotai/kimi-k2.5
cmd --model kimi-k2.5

Docs:  https://commandcode.ai/docs/reference/cli/models
";

    #[test]
    fn listing_rows_become_models_and_prose_is_skipped() {
        let models = parse_models(LISTING).expect("parses");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "deepseek/deepseek-v4-flash",
                "moonshotai/kimi-k2.5",
                "poolside/laguna-s-2.1-free",
                "claude-sonnet-5"
            ]
        );
        assert!(models[1].capabilities.expect("caps").vision);
        assert!(models[0].capabilities.expect("caps").fast);
        // Price is never inferred from the name.
        assert!(models[2].discovered_usd_per_mtok.is_none());
    }

    #[test]
    fn ndjson_error_run_is_reported_as_failure() {
        // Captured from `cmd -p --output-format json` (1.38) on an account
        // without credits.
        let lines = [
            r#"{"type":"event","event":{"type":"notice","level":"info","message":"update available"}}"#,
            r#"{"type":"event","event":{"type":"run_start","sessionId":"8472f122"}}"#,
            r#"{"type":"event","event":{"type":"turn_start","turnNumber":1}}"#,
            r#"{"type":"event","event":{"type":"run_error","error":{"name":"TransportError","message":"insufficient credits"}}}"#,
            r#"{"type":"result","subtype":"error","sessionId":"8472f122","usage":{"inputTokens":0,"outputTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0},"durationMs":1172,"finalText":"","error":"Error: You have insufficient credits"}"#,
        ];
        let mut run = ParsedRun::default();
        for line in lines {
            feed(&mut run, line);
        }
        assert_eq!(run.session_id.as_deref(), Some("8472f122"));
        assert_eq!(run.reported_success, Some(false));
        assert!(
            run.error
                .as_deref()
                .unwrap_or("")
                .contains("insufficient credits")
        );
        assert!(run.texts.is_empty());
        assert_eq!(run.usage.input_tokens, Some(0));
    }

    #[test]
    fn ndjson_success_run_collects_tools_and_answer() {
        let lines = [
            r#"{"type":"event","event":{"type":"run_start","sessionId":"s1"}}"#,
            r#"{"type":"event","event":{"type":"tool_running","name":"read_file"}}"#,
            r#"{"type":"event","event":{"type":"tool_finished","name":"read_file"}}"#,
            r#"{"type":"result","subtype":"success","sessionId":"s1","finalText":"done","usage":{"inputTokens":10,"outputTokens":2}}"#,
        ];
        let mut run = ParsedRun::default();
        let mut events = Vec::new();
        for line in lines {
            events.extend(feed(&mut run, line));
        }
        assert_eq!(run.reported_success, Some(true));
        assert_eq!(run.texts, vec!["done"]);
        assert_eq!(run.tools, vec!["read_file"]);
        assert!(events.contains(&TaskEvent::Tool {
            name: "read_file".to_owned(),
            finished: false
        }));
    }
}
