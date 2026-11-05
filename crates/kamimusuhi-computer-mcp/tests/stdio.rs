use serde_json::{Value, json};
#[cfg(target_os = "macos")]
use std::fs;
use std::{
    io::Write,
    process::{Command, Stdio},
};
fn exchange(mut command: Command, messages: &[Value]) -> Vec<Value> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        for message in messages {
            writeln!(input, "{message}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
#[test]
fn legacy_stdio_contract() {
    let messages = vec![
        json!({"id":1,"method":"initialize","params":{}}),
        json!({"id":2,"method":"tools/list"}),
        json!({"id":3,"method":"ping"}),
        json!({"id":4,"method":"tools/call","params":{"name":"wait","arguments":{"ms":0}}}),
        json!({"id":5,"method":"tools/call","params":{"name":"type_text","arguments":{"text":"あ".repeat(8001)}}}),
        json!({"id":6,"method":"unsupported"}),
        json!({"id":7,"method":"tools/call","params":{"name":"not_a_tool","arguments":{}}}),
    ];
    let mut native = exchange(
        Command::new(env!("CARGO_BIN_EXE_kamimusuhi-computer-mcp")),
        &messages,
    );
    let mut legacy = Command::new("python3");
    legacy.arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/resident/computer-mcp.py"
    ));
    let mut legacy = exchange(legacy, &messages);
    // Python KeyError adds quotes to its display; both must return tool errors.
    for frames in [&mut native, &mut legacy] {
        assert_eq!(frames[6]["result"]["isError"], true);
        assert!(
            frames[6]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool")
        );
        frames[6]["result"]["content"][0]["text"] = json!("unknown tool");
    }
    // JSON encoded inside MCP text has insignificant whitespace/key ordering.
    for frame in &mut legacy {
        if let Some(items) = frame
            .pointer_mut("/result/content")
            .and_then(Value::as_array_mut)
        {
            for item in items {
                if let Some(text) = item["text"].as_str()
                    && let Ok(value) = serde_json::from_str::<Value>(text)
                {
                    item["text"] = Value::String(value.to_string());
                }
            }
        }
    }
    assert_eq!(native, legacy);
}
#[cfg(target_os = "macos")]
#[test]
fn mac_backend_uses_arguments_and_restores_unicode_clipboard() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for (name, script) in [
        (
            "osascript",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$COMPUTER_DIR/calls\"\ncase \"$*\" in *AXIsProcessTrusted*) echo true;; *) echo ok;; esac\n",
        ),
        ("pbpaste", "#!/bin/sh\nprintf 'previous clipboard'\n"),
        (
            "pbcopy",
            "#!/bin/sh\n/bin/cat >> \"$COMPUTER_DIR/clipboard\"\nprintf '\\n' >> \"$COMPUTER_DIR/clipboard\"\n",
        ),
    ] {
        let p = dir.path().join(name);
        fs::write(&p, script).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kamimusuhi-computer-mcp"));
    cmd.env("PATH", dir.path()).env("COMPUTER_DIR", dir.path());
    let frames = exchange(
        cmd,
        &[
            json!({"id":1,"method":"tools/call","params":{"name":"type_text","arguments":{"text":"日本語"}}}),
            json!({"id":2,"method":"tools/call","params":{"name":"key_press","arguments":{"key":"cmd+space"}}}),
            json!({"id":3,"method":"tools/call","params":{"name":"type_text","arguments":{"text":"ASCII literal"}}}),
        ],
    );
    assert_eq!(
        frames[0]["result"]["structuredContent"],
        json!({"typed_chars":3})
    );
    assert_eq!(frames[1]["result"]["isError"], false);
    assert_eq!(
        fs::read_to_string(dir.path().join("clipboard")).unwrap(),
        "日本語\nprevious clipboard\nASCII literal\nprevious clipboard\n"
    );
    let calls = fs::read_to_string(dir.path().join("calls")).unwrap();
    assert!(calls.contains("keystroke \"v\" using {command down}"));
    assert!(calls.contains("key code 49 using {command down}"));
    assert_eq!(frames[2]["result"]["structuredContent"]["typed_chars"], 13);
    assert!(!calls.contains("keystroke \"ASCII literal\""));
}
