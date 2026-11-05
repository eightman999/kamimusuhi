#!/usr/bin/env python3
"""Kamimusuhi computer-use MCP server (stdio, newline JSON-RPC 2.0).

One `computer_<node>` server per node. Observation tools are read-only;
actuation tools are meant to be listed in the resident's `judge_required`
so the judge model screens each call (or `approval_required` for a human
gate).

Backends:
  macOS — osascript (System Events AX tree, CGEvent via the JXA ObjC
          bridge), screencapture, pbcopy/pbpaste, open. No third-party
          dependency; Accessibility and Screen Recording consent belong to
          the resident binary that spawns this process.
  Linux — an X11 display. `DISPLAY` is used when it answers; otherwise a
          dedicated Xvfb on $COMPUTER_DISPLAY (default :99) is spawned and
          kept running so the desktop persists between calls. Input uses
          xdotool; screenshots use scrot or ImageMagick `import`.

Environment:
  COMPUTER_DIR     state dir for screenshots and the Xvfb pidfile
                   (default ~/.local/state/kamimusuhi/computer)
  COMPUTER_DISPLAY Linux display when DISPLAY is unset/dead (default :99)
  COMPUTER_SCREEN  Xvfb screen spec (default 1280x800x24)
  COMPUTER_XVFB    explicit path to an Xvfb binary (else PATH lookup)
"""

import json
import os
import platform
import shlex
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

PROTOCOL_VERSION = "2025-06-18"
SERVER_NAME = "kamimusuhi-computer"
VERSION = "1.0.0"

MAX_TEXT_CHARS = 8000
MAX_WAIT_MS = 10_000
OSASCRIPT_TIMEOUT = 20


def workdir() -> Path:
    base = os.environ.get("COMPUTER_DIR") or str(
        Path.home() / ".local" / "state" / "kamimusuhi" / "computer"
    )
    path = Path(base)
    path.mkdir(parents=True, exist_ok=True)
    return path


def run(argv, timeout=15, env=None):
    """Run argv (no shell); return (rc, stdout, stderr)."""
    merged = dict(os.environ)
    if env:
        merged.update(env)
    try:
        out = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=merged,
        )
        return out.returncode, out.stdout, out.stderr
    except FileNotFoundError:
        return 127, "", f"command not found: {argv[0]}"
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {timeout}s"


# ---------------------------------------------------------------------------
# Linux backend: a persistent X display (Xvfb :99 by default) + xdotool/scrot.
# ---------------------------------------------------------------------------


def _display_alive(display: str) -> bool:
    """A local X display answers when its socket exists (and, when xdotool
    is present, accepts a real query)."""
    if not display:
        return False
    if display.startswith(":"):
        socket = f"/tmp/.X11-unix/X{display[1:].split('.')[0]}"
        if not os.path.exists(socket):
            return False
        if shutil.which("xdotool"):
            env = dict(os.environ, DISPLAY=display)
            rc, out, err = run(["xdotool", "getdisplaygeometry"], timeout=5, env=env)
            if rc != 0:
                print(
                    f"display {display} probe failed rc={rc}: {err.strip()}",
                    file=sys.stderr,
                    flush=True,
                )
            return rc == 0
        return True
    # Networked displays are out of scope for the managed desktop.
    return False


def _xvfb_binary():
    for candidate in (
        os.environ.get("COMPUTER_XVFB"),
        shutil.which("Xvfb"),
        "/srv/kamimusuhi/computer/bin/Xvfb",
    ):
        if candidate and os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def _ensure_display() -> str:
    """Return a live DISPLAY value, spawning the node's Xvfb if needed."""
    existing = os.environ.get("DISPLAY", "")
    if _display_alive(existing):
        return existing
    target = os.environ.get("COMPUTER_DISPLAY", ":99")
    if _display_alive(target):
        return target
    xvfb = _xvfb_binary()
    if not xvfb:
        raise RuntimeError(
            "no X display answers and Xvfb is not installed "
            "(install xvfb, or set COMPUTER_XVFB)"
        )
    screen = os.environ.get("COMPUTER_SCREEN", "1280x800x24")
    number = target[1:].split(".")[0]
    lock = Path(f"/tmp/.X{number}-lock")
    if lock.exists():
        try:
            pid = int(lock.read_text().strip())
            os.kill(pid, 0)
        except (ValueError, OSError):
            lock.unlink(missing_ok=True)
    log = open(workdir() / "xvfb.log", "ab", buffering=0)
    proc = subprocess.Popen(
        [xvfb, target, "-screen", "0", screen, "-nolisten", "tcp"],
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    deadline = time.time() + 10
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"Xvfb exited with {proc.returncode}; see {log.name}")
        if os.path.exists(f"/tmp/.X11-unix/X{number}"):
            (workdir() / "xvfb.pid").write_text(f"{proc.pid}\n")
            os.environ["DISPLAY"] = target
            return target
        time.sleep(0.1)
    proc.terminate()
    raise RuntimeError(f"Xvfb on {target} did not come up in 10s")


def _x(argv, timeout=15):
    display = _ensure_display()
    return run(argv, timeout=timeout, env={"DISPLAY": display})


_KEYMAP_X11 = {
    "return": "Return", "enter": "Return", "tab": "Tab", "escape": "Escape",
    "esc": "Escape", "space": "space", "delete": "BackSpace", "backspace": "BackSpace",
    "forwarddelete": "Delete", "home": "Home", "end": "End",
    "pageup": "Page_Up", "pagedown": "Page_Down", "up": "Up", "down": "Down",
    "left": "Left", "right": "Right",
    "cmd": "super", "command": "super", "option": "alt", "opt": "alt",
    "control": "ctrl",
}
for _i in range(1, 13):
    _KEYMAP_X11[f"f{_i}"] = f"F{_i}"


def _x11_combo(combo: str) -> str:
    parts = [p for p in combo.replace("-", "+").split("+") if p.strip()]
    if not parts:
        raise ValueError("empty key")
    return "+".join(_KEYMAP_X11.get(p.strip().lower(), p.strip()) for p in parts)


def x11_list_windows():
    # wmctrl needs an EWMH window manager; on a bare managed display it
    # exits non-zero, so fall through to xdotool. `xdotool search` exits 1
    # when nothing matches — an empty desktop is an empty list, not an error.
    if shutil.which("wmctrl"):
        rc, out, err = _x(["wmctrl", "-l"])
        if rc == 0:
            windows = []
            for line in out.splitlines():
                fields = line.split(None, 3)
                if len(fields) >= 4:
                    windows.append(
                        {"id": fields[0], "desktop": fields[1], "title": fields[3]}
                    )
            return {"windows": windows, "backend": "wmctrl"}
    rc, out, err = _x(["xdotool", "search", "--onlyvisible", "--name", "."])
    if rc != 0 and out.strip():
        return None
    windows = []
    for wid in out.split():
        rc2, name, _ = _x(["xdotool", "getwindowname", wid])
        windows.append({"id": wid, "title": name.strip() if rc2 == 0 else ""})
    return {"windows": windows, "backend": "xdotool"}


def x11_health():
    display = _ensure_display()
    tools = {name: bool(shutil.which(name)) for name in ("xdotool", "scrot", "import", "wmctrl", "xclip", "Xvfb")}
    pidfile = workdir() / "xvfb.pid"
    xvfb_pid = int(pidfile.read_text().strip()) if pidfile.exists() else None
    return {
        "xvfb_pid": xvfb_pid,
        "platform": "linux",
        "display": display,
        "managed_display": display == os.environ.get("COMPUTER_DISPLAY", ":99")
        or display.startswith(":"),
        "tools": tools,
    }


# ---------------------------------------------------------------------------
# macOS backend: osascript (JXA for CGEvent/AX, AppleScript for keystrokes),
# screencapture, pbcopy/pbpaste, open.
# ---------------------------------------------------------------------------


def _jxa(source, timeout=OSASCRIPT_TIMEOUT):
    return run(["osascript", "-l", "JavaScript", "-e", source], timeout=timeout)


def _as(source, timeout=OSASCRIPT_TIMEOUT):
    return run(["osascript", "-e", source], timeout=timeout)


def _as_quote(text: str) -> str:
    return text.replace("\\", "\\\\").replace('"', '\\"')


_MAC_KEYCODES = {
    "return": 36, "enter": 36, "tab": 48, "space": 49, "delete": 51,
    "backspace": 51, "escape": 53, "esc": 53, "left": 123, "right": 124,
    "down": 125, "up": 126, "home": 115, "end": 119, "pageup": 116,
    "pagedown": 121, "forwarddelete": 117,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
    "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
}
_MAC_MODS = {
    "cmd": "command down", "command": "command down",
    "opt": "option down", "option": "option down", "alt": "option down",
    "ctrl": "control down", "control": "control down", "shift": "shift down",
}


def mac_combo(combo: str):
    """'cmd+space' → (keystroke|key code source fragment)."""
    parts = [p.strip() for p in combo.replace("-", "+").split("+") if p.strip()]
    if not parts:
        raise ValueError("empty key")
    mods = []
    while parts and parts[0].lower() in _MAC_MODS:
        mods.append(_MAC_MODS[parts.pop(0).lower()])
    if not parts:
        raise ValueError("no key in combo")
    key = parts[0]
    using = f' using {{{", ".join(mods)}}}' if mods else ""
    if len(key) == 1 and not key.isdigit():
        return f'keystroke "{_as_quote(key)}"{using}'
    if key.lower() in _MAC_KEYCODES:
        return f"key code {_MAC_KEYCODES[key.lower()]}{using}"
    if key.isdigit():
        return f"key code {key}{using}"
    raise ValueError(f"unknown key {key!r}; use a name, a character or a numeric key code")


def mac_click(x, y, button, clicks):
    kind = {
        "left": ("kCGEventLeftMouseDown", "kCGEventLeftMouseUp", "kCGMouseButtonLeft"),
        "right": ("kCGEventRightMouseDown", "kCGEventRightMouseUp", "kCGMouseButtonRight"),
        "middle": ("kCGEventOtherMouseDown", "kCGEventOtherMouseUp", "kCGMouseButtonCenter"),
    }.get(button)
    if not kind:
        raise ValueError(f"unknown button {button!r}")
    down, up, btn = kind
    src = f"""
ObjC.import('CoreGraphics');
var p = $.CGPointMake({int(x)}, {int(y)});
function ev(t) {{ return $.CGEventCreateMouseEvent($(), t, p, $.{btn}); }}
for (var i = 1; i <= {int(clicks)}; i++) {{
  var d = ev($.{down});
  var u = ev($.{up});
  if (i > 1) {{
    $.CGEventSetIntegerValueField(d, $.kCGMouseEventClickState, i);
    $.CGEventSetIntegerValueField(u, $.kCGMouseEventClickState, i);
  }}
  $.CGEventPost($.kCGHIDEventTap, d);
  $.CGEventPost($.kCGHIDEventTap, u);
  $.CFRunLoopRunInMode($.kCFRunLoopDefaultMode, 0.05, false);
}}
"ok"
"""
    return _jxa(src)


def mac_type_text(text: str):
    if len(text) > MAX_TEXT_CHARS:
        raise ValueError(f"text longer than {MAX_TEXT_CHARS} chars")
    mac_require_ax()
    if all(32 <= ord(c) < 127 for c in text):
        rc, out, err = _as(
            'tell application "System Events" to keystroke "%s"' % _as_quote(text)
        )
        return rc, out, err
    # Non-ASCII: go through the clipboard, then put the old contents back.
    _, old, _ = run(["pbpaste"])
    copy = subprocess.run(["pbcopy"], input=text, text=True)
    if copy.returncode != 0:
        return 1, "", "pbcopy failed"
    rc, out, err = _as(
        'tell application "System Events" to keystroke "v" using {command down}'
    )
    time.sleep(0.2)
    subprocess.run(["pbcopy"], input=old, text=True)
    return rc, out, err


def mac_list_windows():
    mac_require_ax()
    src = r"""
var se = Application('System Events');
var out = [];
try {
  se.processes.whose({backgroundOnly: false})().forEach(function (p) {
    var name; try { name = p.name(); } catch (e) { return; }
    var titles = [];
    try { titles = p.windows.name(); } catch (e) {}
    out.push({app: name, windows: titles || []});
  });
} catch (e) { out.push({error: String(e)}); }
JSON.stringify(out)
"""
    rc, out, err = _jxa(src, timeout=30)
    if rc != 0:
        return None
    try:
        return {"windows": json.loads(out.strip() or "[]"), "backend": "ax"}
    except json.JSONDecodeError:
        return None


def mac_ui_tree(app, depth=3, limit=250):
    """Accessibility elements of an app's windows, bounded by depth/limit."""
    mac_require_ax()
    src = r"""
var se = Application('System Events');
var target = %s;
var DEPTH = %d, LIMIT = %d, seen = 0;
function pick(el) {
  function get(f) { try { return el[f](); } catch (e) { return undefined; } }
  var pos = {}; try { var p = el.position(); pos = {x: p[0], y: p[1]}; } catch (e) {}
  var size = {}; try { var s = el.size(); size = {w: s[0], h: s[1]}; } catch (e) {}
  return {role: get('role'), name: get('name'), title: get('title'),
          description: get('description'), value: get('value'),
          position: pos, size: size};
}
function walk(el, depth, out) {
  if (seen >= LIMIT || depth < 0) return;
  var node = pick(el); seen++;
  var kids = [];
  if (depth > 0 && seen < LIMIT) {
    try {
      el.uiElements().forEach(function (k) {
        if (seen < LIMIT) kids.push(walk(k, depth - 1, out));
      });
    } catch (e) {}
  }
  if (kids.length) node.children = kids;
  return node;
}
var out = [];
try {
  var proc = se.processes.whose({name: target})();
  if (!proc.length) proc = se.processes.whose({_match: [ObjectSpecifier().name, target]})();
  var p = proc[0];
  p.windows().forEach(function (w) { if (seen < LIMIT) out.push(walk(w, DEPTH, out)); });
} catch (e) { JSON.stringify({error: String(e)}); }
JSON.stringify({app: target, elements: out, truncated: seen >= LIMIT})
""" % (json.dumps(app), int(depth), int(limit))
    rc, out, err = _jxa(src, timeout=45)
    if rc != 0:
        return None
    try:
        return json.loads(out.strip() or "{}")
    except json.JSONDecodeError:
        return None


def mac_ax_trusted():
    rc, out, _ = _jxa("ObjC.import('ApplicationServices'); $.AXIsProcessTrusted()")
    return rc == 0 and "true" in out


def mac_require_ax():
    """System Events queries hang without Accessibility consent — fail fast
    with an actionable message instead of waiting out the timeout."""
    if not mac_ax_trusted():
        raise RuntimeError(
            "accessibility permission missing: enable the responsible process "
            "(python3 or the resident) in System Settings > Privacy & Security "
            "> Accessibility, then retry"
        )


def mac_health():
    return {
        "platform": "darwin",
        "display": "aqua",
        "accessibility_trusted": mac_ax_trusted(),
        "tools": {
            "osascript": bool(shutil.which("osascript")),
            "screencapture": bool(shutil.which("screencapture")),
            "pbcopy": bool(shutil.which("pbcopy")),
        },
    }


# ---------------------------------------------------------------------------
# Tool dispatch.
# ---------------------------------------------------------------------------


def _int(args, name, default=None):
    value = args.get(name, default)
    if value is None:
        raise ValueError(f"missing {name}")
    return int(value)


def tool_health_check(_args):
    if platform.system() == "Darwin":
        return mac_health()
    return x11_health()


def tool_list_windows(_args):
    result = mac_list_windows() if platform.system() == "Darwin" else x11_list_windows()
    if result is None:
        raise RuntimeError("window listing failed")
    return result


def tool_ui_tree(args):
    if platform.system() != "Darwin":
        raise RuntimeError("ui_tree is only implemented on macOS; use list_windows and screenshot")
    app = args.get("app") or args.get("name")
    if not app:
        raise ValueError("missing app")
    result = mac_ui_tree(str(app), int(args.get("depth", 3)), int(args.get("limit", 250)))
    if result is None:
        raise RuntimeError("ui_tree failed (accessibility permission?)")
    return result


def tool_screenshot(args):
    directory = workdir() / "screens"
    directory.mkdir(exist_ok=True)
    path = directory / f"screen-{int(time.time() * 1000)}.png"
    if platform.system() == "Darwin":
        argv = ["screencapture", "-x", "-t", "png"]
        if args.get("region"):
            x, y, w, h = (int(v) for v in str(args["region"]).split(","))
            argv += ["-R", f"{x},{y},{w},{h}"]
        argv.append(str(path))
        rc, _, err = run(argv, timeout=15)
    else:
        display = _ensure_display()
        if shutil.which("scrot"):
            rc, _, err = run(["scrot", "-o", str(path)], env={"DISPLAY": display})
        elif shutil.which("import"):
            rc, _, err = run(
                ["import", "-window", "root", str(path)], env={"DISPLAY": display}
            )
        else:
            raise RuntimeError("no screenshot tool (scrot or imagemagick import)")
    if rc != 0:
        raise RuntimeError(err.strip() or "screenshot failed")
    return {"path": str(path), "bytes": path.stat().st_size}


def tool_wait(args):
    ms = min(_int(args, "ms", 500), MAX_WAIT_MS)
    time.sleep(ms / 1000)
    return {"waited_ms": ms}


def tool_mouse_move(args):
    x, y = _int(args, "x"), _int(args, "y")
    if platform.system() == "Darwin":
        rc, out, err = _jxa(
            "ObjC.import('CoreGraphics');"
            "$.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent("
            f"$(), $.kCGEventMouseMoved, $.CGPointMake({x}, {y}), 0));'ok'"
        )
    else:
        rc, out, err = _x(["xdotool", "mousemove", str(x), str(y)])
    if rc != 0:
        raise RuntimeError(err.strip() or "mouse_move failed")
    return {"x": x, "y": y}


def tool_click(args):
    x, y = _int(args, "x"), _int(args, "y")
    button = str(args.get("button", "left"))
    clicks = min(int(args.get("clicks", 1)), 3)
    if platform.system() == "Darwin":
        rc, out, err = mac_click(x, y, button, clicks)
    else:
        buttons = {"left": "1", "middle": "2", "right": "3", "wheel_up": "4", "wheel_down": "5"}
        btn = buttons.get(button)
        if not btn:
            raise ValueError(f"unknown button {button!r}")
        rc, _, err = _x(["xdotool", "mousemove", str(x), str(y)])
        if rc == 0:
            rc, _, err = _x(
                ["xdotool", "click", "--repeat", str(clicks), "--delay", "60", btn]
            )
    if rc != 0:
        raise RuntimeError(err.strip() or "click failed")
    return {"x": x, "y": y, "button": button, "clicks": clicks}


def tool_key_press(args):
    key = str(args.get("key") or "")
    if not key:
        raise ValueError("missing key")
    if platform.system() == "Darwin":
        mac_require_ax()
        fragment = mac_combo(key)
        rc, out, err = _as(f'tell application "System Events" to {fragment}')
    else:
        rc, out, err = _x(["xdotool", "key", "--clearmodifiers", _x11_combo(key)])
    if rc != 0:
        raise RuntimeError(err.strip() or f"key_press {key!r} failed")
    return {"key": key}


def tool_type_text(args):
    text = str(args.get("text") or "")
    if not text:
        raise ValueError("missing text")
    if len(text) > MAX_TEXT_CHARS:
        raise ValueError(f"text longer than {MAX_TEXT_CHARS} chars")
    if platform.system() == "Darwin":
        rc, out, err = mac_type_text(text)
    else:
        rc, out, err = _x(["xdotool", "type", "--clearmodifiers", "--delay", "15", "--", text], timeout=60)
    if rc != 0:
        raise RuntimeError(err.strip() or "type_text failed")
    return {"typed_chars": len(text)}


def tool_scroll(args):
    dy = _int(args, "dy", 0)
    dx = _int(args, "dx", 0)
    if platform.system() == "Darwin":
        src = (
            "ObjC.import('CoreGraphics');"
            "var e = $.CGEventCreateScrollWheelEvent($(), $.kCGScrollEventUnitLine, 2, "
            f"{-int(dy)}, {int(dx)});"
            "$.CGEventPost($.kCGHIDEventTap, e);'ok'"
        )
        rc, out, err = _jxa(src)
    else:
        rc, out, err = 0, "", ""
        if args.get("x") is not None and args.get("y") is not None:
            rc, _, err = _x(["xdotool", "mousemove", str(_int(args, "x")), str(_int(args, "y"))])
        if rc == 0:
            steps_v = min(abs(dy), 40)
            btn = "4" if dy < 0 else "5"
            if steps_v:
                rc, _, err = _x(["xdotool", "click", "--repeat", str(steps_v), btn])
            steps_h = min(abs(dx), 40)
            if rc == 0 and steps_h:
                btn = "6" if dx < 0 else "7"
                rc, _, err = _x(["xdotool", "click", "--repeat", str(steps_h), btn])
    if rc != 0:
        raise RuntimeError(err.strip() or "scroll failed")
    return {"dx": dx, "dy": dy}


def tool_activate(args):
    target = str(args.get("app") or args.get("window") or "")
    if not target:
        raise ValueError("missing app or window")
    if platform.system() == "Darwin":
        mac_require_ax()
        script = (
            f'var se = Application("System Events");'
            f'var procs = se.processes.whose({{name: {{_contains: {json.dumps(target)}}}}})();'
            f'if (procs.length) {{ procs[0].frontmost = true; "ok" }}'
            f' else {{ Application({json.dumps(target)}).activate(); "ok" }}'
        )
        rc, out, err = _jxa(script)
    else:
        rc, out, err = _x(["xdotool", "search", "--name", target, "windowactivate", "%1"])
        if rc != 0:
            rc, out, err = _x(["wmctrl", "-a", target])
    if rc != 0:
        raise RuntimeError(err.strip() or f"activate {target!r} failed")
    return {"activated": target}


def tool_open_application(args):
    name = str(args.get("name") or "")
    if not name:
        raise ValueError("missing name")
    extra = [str(a) for a in args.get("args") or []]
    if platform.system() == "Darwin":
        argv = ["open", name] if name.startswith("/") or name.endswith(".app") else ["open", "-a", name]
        rc, out, err = run(argv + extra, timeout=15)
        if rc != 0:
            raise RuntimeError(err.strip() or f"open {name!r} failed")
        return {"opened": name}
    display = _ensure_display()
    env = dict(os.environ, DISPLAY=display)
    try:
        subprocess.Popen(
            [name] + extra,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=env,
            start_new_session=True,
        )
    except FileNotFoundError:
        raise RuntimeError(f"command not found: {name}")
    return {"opened": name, "display": display}


def _schema(props, required=()):
    return {
        "type": "object",
        "properties": props,
        "required": list(required),
        "additionalProperties": False,
    }


_XY = {"x": {"type": "integer"}, "y": {"type": "integer"}}
READ = {"readOnlyHint": True, "destructiveHint": False}
WRITE = {"readOnlyHint": False}

TOOLS = {
    "health_check": (
        "このノードの computer-use 基盤の状態（プラットフォーム、ディスプレイ、"
        "必要ツールの有無、macOS のアクセシビリティ許可）。副作用なし。",
        _schema({}),
        READ,
        tool_health_check,
    ),
    "list_windows": (
        "画面上のウィンドウ・アプリ一覧（macOS: AX 経由のプロセスとウィンドウ名、"
        "Linux: 管理ディスプレイ上の X ウィンドウ）。",
        _schema({}),
        READ,
        tool_list_windows,
    ),
    "ui_tree": (
        "macOS 専用: 指定アプリのウィンドウ内 UI 要素ツリー（role/name/value/位置）。"
        "深さ depth（既定3）・要素数 limit（既定250）まで。",
        _schema(
            {
                "app": {"type": "string", "description": "アプリ/プロセス名"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 6},
                "limit": {"type": "integer", "minimum": 10, "maximum": 1000},
            },
            ["app"],
        ),
        READ,
        tool_ui_tree,
    ),
    "screenshot": (
        "画面を PNG として保存し、パスを返す。画像自体はモデルには届かない"
        "（記録・操作者確認用）。region に 'x,y,w,h' で範囲指定可。",
        _schema({"region": {"type": "string", "description": "x,y,w,h"}}),
        READ,
        tool_screenshot,
    ),
    "wait": (
        "ms ミリ秒待つ（最大10秒）。画面反映待ちに使う。",
        _schema({"ms": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS}}, ["ms"]),
        READ,
        tool_wait,
    ),
    "mouse_move": (
        "マウスカーソルを座標へ移動する（クリックはしない）。",
        _schema(_XY, ["x", "y"]),
        WRITE,
        tool_mouse_move,
    ),
    "click": (
        "座標 (x,y) をクリック。button: left|middle|right、clicks: 1-3（2でダブルクリック）。",
        _schema(
            dict(
                _XY,
                button={"type": "string", "enum": ["left", "middle", "right", "wheel_up", "wheel_down"]},
                clicks={"type": "integer", "minimum": 1, "maximum": 3},
            ),
            ["x", "y"],
        ),
        WRITE,
        tool_click,
    ),
    "key_press": (
        "キーまたはショートカットを送る。'return','tab','escape','space','delete',矢印,"
        "'f1'-'f12', 修飾子は 'cmd+space' 'ctrl+l' のように + 連結。macOS は key code 数値も可。",
        _schema({"key": {"type": "string"}}, ["key"]),
        WRITE,
        tool_key_press,
    ),
    "type_text": (
        "文字列をそのまま入力する（英数字は直接キー入力、日本語等はクリップボード貼り付け経由）。"
        "フォーカス中の入力欄に入る。",
        _schema({"text": {"type": "string", "maxLength": MAX_TEXT_CHARS}}, ["text"]),
        WRITE,
        tool_type_text,
    ),
    "scroll": (
        "スクロール。dy: 負=上/正=下（行数）、dx: 水平。x,y を指定するとその位置でスクロール。",
        _schema(dict(_XY, dx={"type": "integer"}, dy={"type": "integer"})),
        WRITE,
        tool_scroll,
    ),
    "activate": (
        "アプリまたはウィンドウを前面に出す。app（プロセス名）または window（タイトル部分一致）。",
        _schema(
            {
                "app": {"type": "string"},
                "window": {"type": "string"},
            }
        ),
        WRITE,
        tool_activate,
    ),
    "open_application": (
        "アプリを開く。macOS: アプリ名または /path/App.app。Linux: 管理ディスプレイ上で"
        "コマンド名（+args 配列）として起動。",
        _schema(
            {
                "name": {"type": "string"},
                "args": {"type": "array", "items": {"type": "string"}},
            },
            ["name"],
        ),
        WRITE,
        tool_open_application,
    ),
}


def call_tool(name, arguments):
    entry = TOOLS.get(name)
    if not entry:
        raise KeyError(f"unknown tool {name!r}")
    return entry[3](arguments or {})


# ---------------------------------------------------------------------------
# Newline-delimited JSON-RPC loop (matches the resident's MCP client).
# ---------------------------------------------------------------------------


def reply(message, result=None, error=None):
    out = {"jsonrpc": "2.0", "id": message.get("id")}
    if error is not None:
        if isinstance(error, dict):
            out["error"] = error
        else:
            out["error"] = {"code": -32603, "message": str(error)}
    else:
        out["result"] = result
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def tool_definitions():
    return [
        {
            "name": name,
            "description": description,
            "inputSchema": schema,
            "annotations": annotations,
        }
        for name, (description, schema, annotations, _fn) in TOOLS.items()
    ]


def main():
    # The spawn notice and every tool call go to stderr so stdout carries
    # protocol frames only.
    print(
        f"{SERVER_NAME} {VERSION} on {platform.system()} pid={os.getpid()}",
        file=sys.stderr,
        flush=True,
    )
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = message.get("method")
        if "id" not in message:
            continue
        try:
            if method == "initialize":
                reply(
                    message,
                    {
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": SERVER_NAME, "version": VERSION},
                    },
                )
            elif method == "ping":
                reply(message, {})
            elif method == "tools/list":
                reply(message, {"tools": tool_definitions()})
            elif method == "tools/call":
                params = message.get("params") or {}
                name = params.get("name")
                arguments = params.get("arguments") or {}
                try:
                    result = call_tool(name, arguments)
                    text = json.dumps(result, ensure_ascii=False)
                    reply(
                        message,
                        {
                            "content": [{"type": "text", "text": text}],
                            "structuredContent": result,
                            "isError": False,
                        },
                    )
                except Exception as exc:  # per-call failure is data, not death
                    reply(
                        message,
                        {
                            "content": [{"type": "text", "text": str(exc)}],
                            "isError": True,
                        },
                    )
            else:
                reply(
                    message,
                    error={"code": -32601, "message": f"{method} not supported"},
                )
        except Exception as exc:
            reply(message, error=str(exc))


if __name__ == "__main__":
    main()
