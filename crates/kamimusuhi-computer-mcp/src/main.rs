//! Native stdio computer-use MCP; OS commands remain isolated subprocesses.
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, String>;
const MAX_TEXT: usize = 8000;
const MAC: bool = cfg!(target_os = "macos");
#[derive(Debug)]
struct Output {
    rc: i32,
    out: String,
    err: String,
}
fn run(args: &[String], seconds: u64, display: Option<&str>, input: Option<String>) -> Output {
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = display {
        cmd.env("DISPLAY", d);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Output {
                rc: 127,
                out: String::new(),
                err: format!("{}: {e}", args[0]),
            };
        }
    };
    if let Some(text) = input {
        let mut stdin = child.stdin.take().unwrap();
        thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let rc = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(1),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Output {
                    rc: 124,
                    out: String::new(),
                    err: format!("timed out after {seconds}s"),
                };
            }
        }
    };
    Output {
        rc,
        out: String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned(),
        err: String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned(),
    }
}
fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}
fn checked(o: Output) -> Result<String> {
    if o.rc == 0 {
        Ok(o.out)
    } else {
        Err(if o.err.trim().is_empty() {
            format!("command failed ({})", o.rc)
        } else {
            o.err.trim().into()
        })
    }
}
fn command(parts: &[&str], seconds: u64) -> Output {
    run(&argv(parts), seconds, None, None)
}
fn exists(name: &str) -> bool {
    which(name).is_some()
}
fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && path
                .metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
fn which(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|p| {
        env::split_paths(&p)
            .map(|p| p.join(name))
            .find(|p| executable(p))
    })
}
fn workdir() -> Result<PathBuf> {
    let path = env::var_os("COMPUTER_DIR")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default())
                .join(".local/state/kamimusuhi/computer")
        });
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
fn display_number(display: &str) -> Result<&str> {
    let n = display
        .strip_prefix(':')
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        Err("managed display must be a local :number".into())
    } else {
        Ok(n)
    }
}
fn alive(display: &str) -> bool {
    let Ok(n) = display_number(display) else {
        return false;
    };
    Path::new(&format!("/tmp/.X11-unix/X{n}")).exists()
        && (!exists("xdotool")
            || run(
                &argv(&["xdotool", "getdisplaygeometry"]),
                5,
                Some(display),
                None,
            )
            .rc == 0)
}
fn display() -> Result<String> {
    let current = env::var("DISPLAY").unwrap_or_default();
    if alive(&current) {
        return Ok(current);
    }
    let target = env::var("COMPUTER_DISPLAY").unwrap_or(":99".into());
    let n = display_number(&target)?;
    if alive(&target) {
        return Ok(target);
    }
    let binary = env::var_os("COMPUTER_XVFB")
        .map(PathBuf::from)
        .filter(|p| executable(p))
        .or_else(|| which("Xvfb"))
        .or_else(|| {
            let p = PathBuf::from("/srv/kamimusuhi/computer/bin/Xvfb");
            executable(&p).then_some(p)
        })
        .ok_or(
            "no X display answers and Xvfb is not installed (install xvfb, or set COMPUTER_XVFB)",
        )?;
    let lock = PathBuf::from(format!("/tmp/.X{n}-lock"));
    if lock.exists() {
        let pid = fs::read_to_string(&lock).unwrap_or_default();
        if pid
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|p| *p > 0)
            .is_none_or(|_| command(&["kill", "-0", pid.trim()], 2).rc != 0)
        {
            fs::remove_file(&lock).map_err(|e| e.to_string())?;
        }
    }
    let dir = workdir()?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("xvfb.log"))
        .map_err(|e| e.to_string())?;
    let mut cmd = Command::new(binary);
    cmd.args([
        &target,
        "-screen",
        "0",
        &env::var("COMPUTER_SCREEN").unwrap_or("1280x800x24".into()),
        "-nolisten",
        "tcp",
    ])
    .stdin(Stdio::null())
    .stderr(log.try_clone().map_err(|e| e.to_string())?)
    .stdout(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("Xvfb exited {s}; see xvfb.log"));
        }
        if Path::new(&format!("/tmp/.X11-unix/X{n}")).exists() {
            fs::write(dir.join("xvfb.pid"), format!("{}\n", child.id()))
                .map_err(|e| e.to_string())?;
            return Ok(target);
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(format!("Xvfb on {target} did not come up in 10s"))
}
fn x(parts: &[&str], seconds: u64) -> Result<Output> {
    Ok(run(&argv(parts), seconds, Some(&display()?), None))
}
fn jxa(src: &str, seconds: u64) -> Output {
    command(&["osascript", "-l", "JavaScript", "-e", src], seconds)
}
fn applescript(src: &str) -> Output {
    command(&["osascript", "-e", src], 20)
}
fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
fn ax_trusted() -> bool {
    let o = jxa(
        "ObjC.import('ApplicationServices'); $.AXIsProcessTrusted()",
        20,
    );
    o.rc == 0 && o.out.contains("true")
}
fn require_ax() -> Result<()> {
    if ax_trusted() {
        Ok(())
    } else {
        Err("accessibility permission missing: enable the responsible process (kamimusuhi-computer-mcp or the resident) in System Settings > Privacy & Security > Accessibility, then retry".into())
    }
}
fn number(a: &Value, n: &str, d: Option<i64>) -> Result<i64> {
    a.get(n)
        .and_then(Value::as_i64)
        .or(d)
        .ok_or_else(|| format!("missing or non-integer {n}"))
}
fn text<'a>(a: &'a Value, n: &str) -> Result<&'a str> {
    a.get(n)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing {n}"))
}
fn mac_combo(combo: &str) -> Result<String> {
    let replaced = combo.replace('-', "+");
    let mut parts = replaced
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .peekable();
    let mut mods = Vec::new();
    while let Some(p) = parts.peek() {
        let m = match p.to_lowercase().as_str() {
            "cmd" | "command" => "command down",
            "opt" | "option" | "alt" => "option down",
            "ctrl" | "control" => "control down",
            "shift" => "shift down",
            _ => break,
        };
        mods.push(m);
        parts.next();
    }
    let key = parts.next().ok_or("no key in combo")?;
    let using = if mods.is_empty() {
        String::new()
    } else {
        format!(" using {{{}}}", mods.join(", "))
    };
    if key.chars().count() == 1 && !key.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(format!("keystroke \"{}\"{using}", quote(key)));
    }
    let code = match key.to_lowercase().as_str() {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        "forwarddelete" => 117,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        _ => key
            .parse::<u16>()
            .map_err(|_| format!("unknown key {key:?}"))?,
    };
    Ok(format!("key code {code}{using}"))
}
fn x_combo(combo: &str) -> Result<String> {
    let result = combo
        .replace('-', "+")
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| match p.to_lowercase().as_str() {
            "return" | "enter" => "Return".into(),
            "tab" => "Tab".into(),
            "escape" | "esc" => "Escape".into(),
            "space" => "space".into(),
            "delete" | "backspace" => "BackSpace".into(),
            "forwarddelete" => "Delete".into(),
            "home" => "Home".into(),
            "end" => "End".into(),
            "pageup" => "Page_Up".into(),
            "pagedown" => "Page_Down".into(),
            "up" => "Up".into(),
            "down" => "Down".into(),
            "left" => "Left".into(),
            "right" => "Right".into(),
            "cmd" | "command" => "super".into(),
            "option" | "opt" => "alt".into(),
            "control" => "ctrl".into(),
            lower
                if lower
                    .strip_prefix('f')
                    .and_then(|n| n.parse::<u8>().ok())
                    .is_some_and(|n| (1..=12).contains(&n)) =>
            {
                lower.to_uppercase()
            }
            _ => p.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("+");
    if result.is_empty() {
        Err("empty key".into())
    } else {
        Ok(result)
    }
}
fn tool(name: &str, a: &Value) -> Result<Value> {
    match name {
        "health_check" => {
            if MAC {
                Ok(
                    json!({"platform":"darwin","display":"aqua","accessibility_trusted":ax_trusted(),"tools":{"osascript":exists("osascript"),"screencapture":exists("screencapture"),"pbcopy":exists("pbcopy")}}),
                )
            } else {
                let d = display()?;
                let mut tools = serde_json::Map::new();
                for n in ["xdotool", "scrot", "import", "wmctrl", "xclip", "Xvfb"] {
                    tools.insert(n.into(), json!(exists(n)));
                }
                let pid = fs::read_to_string(workdir()?.join("xvfb.pid"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                Ok(
                    json!({"platform":"linux","display":d,"managed_display":d.starts_with(':'),"xvfb_pid":pid,"tools":tools}),
                )
            }
        }
        "list_windows" => {
            if MAC {
                require_ax()?;
                let o = checked(jxa(include_str!("mac_list_windows.js"), 30))?;
                return Ok(
                    json!({"windows":serde_json::from_str::<Value>(o.trim()).map_err(|e|e.to_string())?,"backend":"ax"}),
                );
            }
            if exists("wmctrl") {
                let o = x(&["wmctrl", "-l"], 15)?;
                if o.rc == 0 {
                    let windows = o
                        .out
                        .lines()
                        .filter_map(|s| {
                            let mut fields = s.split_whitespace();
                            let id = fields.next()?;
                            let desktop = fields.next()?;
                            fields.next()?;
                            let title = fields.collect::<Vec<_>>().join(" ");
                            Some(json!({"id":id,"desktop":desktop,"title":title}))
                        })
                        .collect::<Vec<_>>();
                    return Ok(json!({"windows":windows,"backend":"wmctrl"}));
                }
            }
            let o = x(&["xdotool", "search", "--onlyvisible", "--name", "."], 15)?;
            if o.rc != 0 && (o.rc != 1 || !o.out.trim().is_empty()) {
                return Err("window listing failed".into());
            }
            let mut windows = Vec::new();
            for id in o.out.split_whitespace() {
                let o = x(&["xdotool", "getwindowname", id], 15)?;
                windows.push(json!({"id":id,"title":if o.rc==0{o.out.trim()}else{""}}));
            }
            Ok(json!({"windows":windows,"backend":"xdotool"}))
        }
        "ui_tree" => {
            if !MAC {
                return Err(
                    "ui_tree is only implemented on macOS; use list_windows and screenshot".into(),
                );
            }
            require_ax()?;
            let app = a
                .get("app")
                .or_else(|| a.get("name"))
                .and_then(Value::as_str)
                .ok_or("missing app")?;
            let depth = number(a, "depth", Some(3))?.clamp(1, 6);
            let limit = number(a, "limit", Some(250))?.clamp(10, 1000);
            let src = include_str!("mac_ui_tree.js")
                .replace("__APP__", &json!(app).to_string())
                .replace("__BOUND__", &depth.to_string())
                .replace("__LIMIT__", &limit.to_string());
            let o = checked(jxa(&src, 45))?;
            serde_json::from_str(o.trim()).map_err(|e| e.to_string())
        }
        "wait" => {
            let ms = number(a, "ms", Some(500))?.clamp(0, 10000);
            thread::sleep(Duration::from_millis(ms as u64));
            Ok(json!({"waited_ms":ms}))
        }
        "screenshot" => {
            let dir = workdir()?.join("screens");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let path = dir.join(format!("screen-{now}.png"));
            let path_s = path.to_string_lossy();
            let o = if MAC {
                let mut args = argv(&["screencapture", "-x", "-t", "png"]);
                if let Some(region) = a
                    .get("region")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    let coords = region
                        .split(',')
                        .map(|s| s.parse::<i64>())
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    if coords.len() != 4 {
                        return Err("region must be x,y,w,h".into());
                    }
                    args.extend(argv(&["-R", region]));
                }
                args.push(path_s.into_owned());
                run(&args, 15, None, None)
            } else {
                let d = display()?;
                let args = if exists("scrot") {
                    argv(&["scrot", "-o", &path_s])
                } else if exists("import") {
                    argv(&["import", "-window", "root", &path_s])
                } else {
                    return Err("no screenshot tool (scrot or imagemagick import)".into());
                };
                run(&args, 15, Some(&d), None)
            };
            checked(o)?;
            Ok(json!({"path":path,"bytes":fs::metadata(&path).map_err(|e|e.to_string())?.len()}))
        }
        "mouse_move" => {
            let x_ = number(a, "x", None)?;
            let y = number(a, "y", None)?;
            let o = if MAC {
                jxa(
                    &format!(
                        "ObjC.import('CoreGraphics');$.CGEventPost($.kCGHIDEventTap,$.CGEventCreateMouseEvent($(),$.kCGEventMouseMoved,$.CGPointMake({x_},{y}),0));'ok'"
                    ),
                    20,
                )
            } else {
                x(
                    &["xdotool", "mousemove", &x_.to_string(), &y.to_string()],
                    15,
                )?
            };
            checked(o)?;
            Ok(json!({"x":x_,"y":y}))
        }
        "click" => {
            let xx = number(a, "x", None)?;
            let y = number(a, "y", None)?;
            let button = a.get("button").and_then(Value::as_str).unwrap_or("left");
            let clicks = number(a, "clicks", Some(1))?.clamp(1, 3);
            let o = if MAC {
                let (down, up, btn) = match button {
                    "left" => (
                        "kCGEventLeftMouseDown",
                        "kCGEventLeftMouseUp",
                        "kCGMouseButtonLeft",
                    ),
                    "right" => (
                        "kCGEventRightMouseDown",
                        "kCGEventRightMouseUp",
                        "kCGMouseButtonRight",
                    ),
                    "middle" => (
                        "kCGEventOtherMouseDown",
                        "kCGEventOtherMouseUp",
                        "kCGMouseButtonCenter",
                    ),
                    _ => return Err(format!("unknown button {button}")),
                };
                jxa(
                    &format!(
                        "ObjC.import('CoreGraphics');var p=$.CGPointMake({xx},{y});for(var i=1;i<={clicks};i++){{var d=$.CGEventCreateMouseEvent($(),$.{down},p,$.{btn});var u=$.CGEventCreateMouseEvent($(),$.{up},p,$.{btn});if(i>1){{$.CGEventSetIntegerValueField(d,$.kCGMouseEventClickState,i);$.CGEventSetIntegerValueField(u,$.kCGMouseEventClickState,i);}}$.CGEventPost($.kCGHIDEventTap,d);$.CGEventPost($.kCGHIDEventTap,u);$.CFRunLoopRunInMode($.kCFRunLoopDefaultMode,0.05,false);}}'ok'"
                    ),
                    20,
                )
            } else {
                let btn = match button {
                    "left" => "1",
                    "middle" => "2",
                    "right" => "3",
                    "wheel_up" => "4",
                    "wheel_down" => "5",
                    _ => return Err(format!("unknown button {button}")),
                };
                checked(x(
                    &["xdotool", "mousemove", &xx.to_string(), &y.to_string()],
                    15,
                )?)?;
                x(
                    &[
                        "xdotool",
                        "click",
                        "--repeat",
                        &clicks.to_string(),
                        "--delay",
                        "60",
                        btn,
                    ],
                    15,
                )?
            };
            checked(o)?;
            Ok(json!({"x":xx,"y":y,"button":button,"clicks":clicks}))
        }
        "key_press" => {
            let key = text(a, "key")?;
            let o = if MAC {
                require_ax()?;
                applescript(&format!(
                    "tell application \"System Events\" to {}",
                    mac_combo(key)?
                ))
            } else {
                x(&["xdotool", "key", "--clearmodifiers", &x_combo(key)?], 15)?
            };
            checked(o)?;
            Ok(json!({"key":key}))
        }
        "type_text" => {
            let text = text(a, "text")?;
            let count = text.chars().count();
            if count > MAX_TEXT {
                return Err(format!("text longer than {MAX_TEXT} chars"));
            }
            let o = if MAC {
                require_ax()?;
                // Keystrokes are interpreted by the active IME even for ASCII.
                // Paste literal text for every input source, then restore the
                // previous plain-text clipboard after the application consumes it.
                let old = checked(command(&["pbpaste"], 15))?;
                checked(run(&argv(&["pbcopy"]), 15, None, Some(text.into())))?;
                let o = applescript(
                    "tell application \"System Events\" to keystroke \"v\" using {command down}",
                );
                thread::sleep(Duration::from_millis(500));
                checked(run(&argv(&["pbcopy"]), 15, None, Some(old)))?;
                o
            } else {
                x(
                    &[
                        "xdotool",
                        "type",
                        "--clearmodifiers",
                        "--delay",
                        "15",
                        "--",
                        text,
                    ],
                    60,
                )?
            };
            checked(o)?;
            Ok(json!({"typed_chars":count}))
        }
        "scroll" => {
            let dy = number(a, "dy", Some(0))?;
            let dx = number(a, "dx", Some(0))?;
            if MAC {
                checked(jxa(
                    &format!(
                        "ObjC.import('CoreGraphics');var e=$.CGEventCreateScrollWheelEvent($(),$.kCGScrollEventUnitLine,2,{},{});$.CGEventPost($.kCGHIDEventTap,e);'ok'",
                        dy.saturating_neg(),
                        dx
                    ),
                    20,
                ))?;
            } else {
                if a.get("x").is_some() && a.get("y").is_some() {
                    checked(x(
                        &[
                            "xdotool",
                            "mousemove",
                            &number(a, "x", None)?.to_string(),
                            &number(a, "y", None)?.to_string(),
                        ],
                        15,
                    )?)?;
                }
                for (steps, button) in [
                    (dy.unsigned_abs().min(40), if dy < 0 { "4" } else { "5" }),
                    (dx.unsigned_abs().min(40), if dx < 0 { "6" } else { "7" }),
                ] {
                    if steps > 0 {
                        checked(x(
                            &["xdotool", "click", "--repeat", &steps.to_string(), button],
                            15,
                        )?)?;
                    }
                }
            }
            Ok(json!({"dx":dx,"dy":dy}))
        }
        "activate" => {
            let target = a
                .get("app")
                .or_else(|| a.get("window"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or("missing app or window")?;
            let o = if MAC {
                require_ax()?;
                let target = json!(target);
                jxa(
                    &format!(
                        "var se=Application('System Events');var procs=se.processes.whose({{name:{{_contains:{target}}}}})();if(procs.length){{procs[0].frontmost=true;'ok'}}else{{Application({target}).activate();'ok'}}"
                    ),
                    20,
                )
            } else {
                let o = x(
                    &[
                        "xdotool",
                        "search",
                        "--name",
                        target,
                        "windowactivate",
                        "%1",
                    ],
                    15,
                )?;
                if o.rc == 0 {
                    o
                } else {
                    x(&["wmctrl", "-a", target], 15)?
                }
            };
            checked(o)?;
            Ok(json!({"activated":target}))
        }
        "open_application" => {
            let name = text(a, "name")?;
            let extra = a
                .get("args")
                .and_then(Value::as_array)
                .map(|xs| {
                    xs.iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .ok_or("args must be strings".to_owned())
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            if MAC {
                let mut args = if name.starts_with('/') || name.ends_with(".app") {
                    argv(&["open", name])
                } else {
                    argv(&["open", "-a", name])
                };
                args.extend(extra);
                checked(run(&args, 15, None, None))?;
                Ok(json!({"opened":name}))
            } else {
                let d = display()?;
                let mut cmd = Command::new(name);
                cmd.args(extra)
                    .env("DISPLAY", &d)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    cmd.process_group(0);
                }
                let mut child = cmd.spawn().map_err(|e| e.to_string())?;
                thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(json!({"opened":name,"display":d}))
            }
        }
        _ => Err(format!("unknown tool {name:?}")),
    }
}
fn response(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"kamimusuhi-computer","version":"1.0.0"}})
        }
        "ping" => json!({}),
        "tools/list" => {
            json!({"tools":serde_json::from_str::<Value>(include_str!("tools.json")).expect("embedded tool catalog")})
        }
        "tools/call" => {
            let params = &message["params"];
            let name = params["name"].as_str().unwrap_or("");
            let empty = json!({});
            let args = params
                .get("arguments")
                .filter(|v| !v.is_null())
                .unwrap_or(&empty);
            match tool(name, args) {
                Ok(result) => {
                    json!({"content":[{"type":"text","text":result.to_string()}],"structuredContent":result,"isError":false})
                }
                Err(error) => json!({"content":[{"type":"text","text":error}],"isError":true}),
            }
        }
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("{method} not supported")}}),
            );
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}
fn main() {
    eprintln!(
        "kamimusuhi-computer 1.0.0 on {} pid={}",
        env::consts::OS,
        std::process::id()
    );
    let mut stdout = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(reply) = response(&message)
            && writeln!(stdout, "{reply}")
                .and_then(|_| stdout.flush())
                .is_err()
        {
            break;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_errors_do_not_stop_next_request() {
        assert!(response(&json!({"method":"notifications/initialized"})).is_none());
        assert_eq!(
            response(&json!({"id":1,"method":"missing"})).unwrap()["error"]["code"],
            -32601
        );
        let error =
            response(&json!({"id":2,"method":"tools/call","params":{"name":"unknown"}})).unwrap();
        assert_eq!(error["result"]["isError"], true);
        assert_eq!(
            response(&json!({"id":3,"method":"ping"})).unwrap()["result"],
            json!({})
        );
    }
    #[test]
    fn shortcuts_and_script_escaping() {
        assert_eq!(
            mac_combo("cmd+shift+space").unwrap(),
            "key code 49 using {command down, shift down}"
        );
        assert_eq!(
            mac_combo("ctrl+\"").unwrap(),
            "keystroke \"\\\"\" using {control down}"
        );
        assert_eq!(x_combo("cmd+pageup").unwrap(), "super+Page_Up");
        assert!(mac_combo("ctrl").is_err());
        assert!(x_combo("").is_err());
    }
    #[test]
    fn bounded_arguments() {
        assert_eq!(
            tool("wait", &json!({"ms":0})).unwrap(),
            json!({"waited_ms":0})
        );
        assert!(tool("type_text", &json!({"text":"あ".repeat(8001)})).is_err());
        assert!(tool("mouse_move", &json!({"x":"oops","y":0})).is_err());
        assert!(display_number("../../file").is_err());
        assert!(display_number(":99.0").is_ok());
    }
    #[test]
    fn process_timeout_and_no_shell() {
        let o = command(&["/bin/sh", "-c", "sleep 2"], 0);
        assert_eq!(o.rc, 124);
        let o = command(&["/bin/echo", "$(false); hello"], 2);
        assert_eq!(o.out.trim(), "$(false); hello");
    }
}
