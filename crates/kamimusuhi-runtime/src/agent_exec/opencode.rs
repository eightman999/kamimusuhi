//! OpenCode preset, model-list parser and `run --format json` events.
//!
//! OpenCode itself costs nothing, but the provider under a model may bill
//! per token, so billing is decided per `provider/model`, never per
//! harness. To avoid a cold start per task, point `extra_args` at a
//! supervised `opencode serve` (`["--attach", "http://127.0.0.1:4096"]`).

use serde_json::Value;

use super::catalog::RawModel;
use super::config::{
    AcpSpec, DiscoverFormat, DiscoverSpec, OutputFormat, PermissionArgs, PermissionModes, Preset,
    RunSpec, ServerSpec, strings,
};
use super::output::ParsedRun;
use super::types::{ModelCapabilities, TaskEvent, TaskKind, TaskUsage};

pub fn preset() -> Preset {
    Preset {
        command: "opencode",
        discover: Some(DiscoverSpec {
            args: strings(&["models", "--refresh", "--verbose"]),
            format: DiscoverFormat::OpencodeVerbose,
        }),
        run: Some(RunSpec {
            prefix: strings(&["run"]),
            args: strings(&["--format", "json", "--dir", "{workspace}", "{prompt}"]),
            model_args: strings(&["--model", "{model}"]),
            resume_args: strings(&["--session", "{session}"]),
            output: OutputFormat::OpencodeJson,
        }),
        // The built-in `plan` agent has edit and bash tools disabled.
        permission_args: PermissionArgs {
            read_only: Some(strings(&["--agent", "plan"])),
            workspace_write: Some(strings(&["--agent", "build"])),
            autonomous_workspace: Some(strings(&["--agent", "build", "--auto"])),
        },
        prefer_for: vec![
            TaskKind::Research,
            TaskKind::Review,
            TaskKind::Planning,
            TaskKind::General,
        ],
        // `opencode acp`: one server for every model (per-session `model`
        // option); `plan` is the read-only agent.
        acp: Some(AcpSpec {
            args: strings(&["acp"]),
            model_args: Vec::new(),
            model_option: Some("model".to_owned()),
            mode_option: "mode".to_owned(),
            modes: PermissionModes {
                read_only: Some("plan".to_owned()),
                workspace_write: Some("build".to_owned()),
                autonomous_workspace: Some("build".to_owned()),
            },
            idle_secs: 900,
        }),
        server: Some(ServerSpec {
            args: strings(&["serve", "--port", "{port}", "--hostname", "127.0.0.1"]),
            port: 4096,
            attach_args: strings(&["--attach", "{url}"]),
            password_env: Some("OPENCODE_SERVER_PASSWORD".to_owned()),
        }),
    }
}

fn is_header(line: &str) -> bool {
    !line.is_empty()
        && !line.starts_with(char::is_whitespace)
        && !line.starts_with(['{', '}'])
        && line.contains('/')
        && !line.contains(' ')
}

fn model_from_json(id: String, meta: &Value) -> RawModel {
    let caps = &meta["capabilities"];
    let context = meta["limit"]["context"].as_u64();
    let lower = id.to_lowercase();
    let capabilities = ModelCapabilities {
        coding: true,
        reasoning: caps["reasoning"].as_bool().unwrap_or(false),
        vision: caps["input"]["image"].as_bool().unwrap_or(false),
        tools: caps["toolcall"].as_bool().unwrap_or(false),
        long_context: context.is_some_and(|c| c >= 200_000),
        fast: super::catalog::sounds_fast(&lower),
    };
    let price = match (
        meta["cost"]["input"].as_f64(),
        meta["cost"]["output"].as_f64(),
    ) {
        (Some(i), Some(o)) => Some((i, o)),
        _ => None,
    };
    RawModel {
        label: meta["name"].as_str().map(str::to_owned),
        context_tokens: context,
        price_note: price.map(|(i, o)| format!("${i} / MTok In · ${o} / MTok Out")),
        discovered_usd_per_mtok: price,
        capabilities: Some(capabilities),
        available: meta["status"].as_str() != Some("deprecated"),
        ..RawModel::new(id)
    }
}

/// `opencode models [--verbose]`: `provider/model` lines, each optionally
/// followed by a pretty-printed JSON object.
pub fn parse_models(text: &str) -> Result<Vec<RawModel>, String> {
    let mut models = Vec::new();
    let mut header: Option<String> = None;
    let mut body = String::new();
    let flush = |header: &mut Option<String>, body: &mut String, models: &mut Vec<RawModel>| {
        if let Some(id) = header.take() {
            let meta = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
            models.push(if meta.is_object() {
                model_from_json(id, &meta)
            } else {
                RawModel::new(id)
            });
        }
        body.clear();
    };
    for line in text.lines() {
        // JSON bodies are pretty-printed, so only headers and the outer
        // braces start at column 0.
        if is_header(line) {
            flush(&mut header, &mut body, &mut models);
            header = Some(line.trim().to_owned());
        } else if header.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&mut header, &mut body, &mut models);
    if models.is_empty() {
        return Err("opencode models: no provider/model lines".to_owned());
    }
    Ok(models)
}

/// One line of `opencode run --format json`.
pub fn feed(run: &mut ParsedRun, line: &str) -> Vec<TaskEvent> {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    if run.session_id.is_none()
        && let Some(id) = event["sessionID"].as_str()
    {
        run.session_id = Some(id.to_owned());
        events.push(TaskEvent::Session { id: id.to_owned() });
    }
    let part = &event["part"];
    match event["type"].as_str().unwrap_or("") {
        "text" => {
            if let Some(text) = part["text"].as_str().filter(|t| !t.trim().is_empty()) {
                run.texts.push(text.to_owned());
            }
        }
        "tool_use" => {
            let name = part["tool"].as_str().unwrap_or("tool").to_owned();
            let finished = matches!(
                part["state"]["status"].as_str(),
                Some("completed" | "error")
            );
            if finished {
                run.tools.push(name.clone());
            }
            events.push(TaskEvent::Tool { name, finished });
        }
        "step_finish" => {
            let tokens = &part["tokens"];
            run.usage.add(TaskUsage {
                input_tokens: tokens["input"].as_u64(),
                output_tokens: tokens["output"]
                    .as_u64()
                    .map(|o| o + tokens["reasoning"].as_u64().unwrap_or(0)),
                cache_read_tokens: tokens["cache"]["read"].as_u64(),
                cache_write_tokens: tokens["cache"]["write"].as_u64(),
            });
            if let Some(cost) = part["cost"].as_f64() {
                run.cost_usd = Some(run.cost_usd.unwrap_or(0.0) + cost);
            }
        }
        "error" => {
            let message = event["error"]["data"]["message"]
                .as_str()
                .or_else(|| event["error"]["message"].as_str())
                .or_else(|| event["error"].as_str())
                .unwrap_or("opencode reported an error");
            run.error = Some(message.to_owned());
            run.reported_success = Some(false);
        }
        _ => {}
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERBOSE: &str = r#"opencode/big-pickle
{
  "id": "big-pickle",
  "providerID": "opencode",
  "name": "Big Pickle",
  "status": "active",
  "cost": {
    "input": 0,
    "output": 0,
    "cache": {
      "read": 0,
      "write": 0
    }
  },
  "limit": {
    "context": 200000,
    "output": 32000
  },
  "capabilities": {
    "reasoning": true,
    "toolcall": true,
    "input": {
      "text": true,
      "image": false
    }
  }
}
anthropic/claude-opus-5-5
{
  "id": "claude-opus-5-5",
  "name": "Claude Opus 5.5",
  "cost": {
    "input": 4,
    "output": 20
  },
  "limit": {
    "context": 1000000
  },
  "capabilities": {
    "reasoning": true,
    "toolcall": true,
    "input": {
      "image": true
    }
  }
}
"#;

    #[test]
    fn verbose_listing_keeps_prices_and_capabilities() {
        let models = parse_models(VERBOSE).expect("parses");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "opencode/big-pickle");
        assert_eq!(models[0].discovered_usd_per_mtok, Some((0.0, 0.0)));
        let caps = models[0].capabilities.expect("caps");
        assert!(caps.tools && caps.reasoning && caps.long_context && !caps.vision);
        assert_eq!(models[1].id, "anthropic/claude-opus-5-5");
        assert_eq!(models[1].discovered_usd_per_mtok, Some((4.0, 20.0)));
        assert!(models[1].capabilities.expect("caps").vision);
    }

    #[test]
    fn plain_listing_parses_too() {
        let models = parse_models("opencode/big-pickle\nlmstudio/qwen3\n").expect("parses");
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].id, "lmstudio/qwen3");
        assert!(models[1].discovered_usd_per_mtok.is_none());
    }

    #[test]
    fn run_events_collect_answer_usage_and_tools() {
        // Shapes captured from `opencode run --format json` (1.18).
        let lines = [
            r#"{"type":"step_start","sessionID":"ses_1","part":{"type":"step-start"}}"#,
            r#"{"type":"tool_use","sessionID":"ses_1","part":{"type":"tool","tool":"read","state":{"status":"completed"}}}"#,
            r#"{"type":"step_finish","sessionID":"ses_1","part":{"reason":"tool-calls","tokens":{"total":23403,"input":23287,"output":100,"reasoning":16,"cache":{"write":0,"read":0}},"cost":0}}"#,
            r#"{"type":"text","sessionID":"ses_1","part":{"type":"text","text":"hello"}}"#,
            r#"{"type":"step_finish","sessionID":"ses_1","part":{"reason":"stop","tokens":{"input":293,"output":3,"reasoning":11,"cache":{"write":0,"read":23232}},"cost":0}}"#,
        ];
        let mut run = ParsedRun::default();
        let mut events = Vec::new();
        for line in lines {
            events.extend(feed(&mut run, line));
        }
        assert_eq!(run.session_id.as_deref(), Some("ses_1"));
        assert_eq!(run.texts, vec!["hello"]);
        assert_eq!(run.tools, vec!["read"]);
        assert_eq!(run.usage.input_tokens, Some(23_580));
        assert_eq!(run.usage.output_tokens, Some(130));
        assert_eq!(run.usage.cache_read_tokens, Some(23_232));
        assert_eq!(run.cost_usd, Some(0.0));
        assert!(events.contains(&TaskEvent::Session {
            id: "ses_1".to_owned()
        }));
    }
}
