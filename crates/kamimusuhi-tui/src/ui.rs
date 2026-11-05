//! Rendering: dark navy palette shared with the desktop app.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Entry, InputMode, View};

pub const BG: Color = Color::Rgb(8, 15, 24);
pub const PANEL: Color = Color::Rgb(13, 24, 36);
pub const PANEL_RAISED: Color = Color::Rgb(19, 32, 47);
pub const BORDER: Color = Color::Rgb(37, 57, 78);
pub const TEXT: Color = Color::Rgb(224, 233, 244);
pub const MUTED: Color = Color::Rgb(139, 160, 185);
pub const BLUE: Color = Color::Rgb(49, 129, 235);
pub const GREEN: Color = Color::Rgb(47, 206, 132);
pub const AMBER: Color = Color::Rgb(243, 181, 62);
pub const RED: Color = Color::Rgb(235, 96, 110);
pub const VIOLET: Color = Color::Rgb(166, 138, 255);
pub const SELECT_BG: Color = Color::Rgb(24, 62, 111);

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn bold(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "-".to_owned(),
        Some(other) => other.to_string(),
    }
}

/// Pad a line with spaces so a row background spans the full width.
pub fn pad(mut line: Line<'static>, width: usize) -> Line<'static> {
    let w = line.width();
    if w < width {
        line.spans.push(Span::raw(" ".repeat(width - w)));
    }
    line
}

/// Scroll offset clamped to `lines - viewport`.
pub fn clamp_scroll(lines: usize, height: u16, scroll: u16) -> u16 {
    let max = u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .saturating_sub(height);
    scroll.min(max)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);
    top_bar(frame, rows[0], app);
    tabs(frame, rows[1], app);
    match app.view {
        View::Chat => chat(frame, rows[2], app),
        View::Tasks => tasks(frame, rows[2], app),
        View::Approvals => approvals(frame, rows[2], app),
        View::Status => status(frame, rows[2], app),
        View::Tools => tools(frame, rows[2], app),
        View::Agents => crate::agents::draw(frame, rows[2], app),
    }
    footer(frame, rows[3], app);
}

fn top_bar(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::horizontal([Constraint::Min(20), Constraint::Length(28)]).split(area);
    let node_line = app.status.as_ref().map_or_else(
        || "接続中…".to_owned(),
        |s| {
            format!(
                "{} · 経路 {} · subject {}",
                s["node"]["node"].as_str().unwrap_or("?"),
                s["routing"]["active_primary"].as_str().unwrap_or("なし"),
                app.subject
            )
        },
    );
    let left = Line::from(vec![
        Span::styled("澪 ", bold(TEXT)),
        Span::styled("常駐個体  ", muted()),
        Span::styled(node_line, muted()),
    ]);
    frame.render_widget(Paragraph::new(left), chunks[0]);
    let (label, color) = match (&app.url, app.busy_since) {
        (None, _) => ("○ 接続待ち".to_owned(), MUTED),
        (Some(_), Some(t)) => (format!("考え中 {}s", t.elapsed().as_secs()), AMBER),
        (Some(_), None) => ("● オンライン".to_owned(), GREEN),
    };
    let mut spans = vec![Span::styled(label, Style::default().fg(color))];
    let pending = app.pending().len();
    if pending > 0 {
        spans.push(Span::styled(format!("  承認 {pending}"), bold(AMBER)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans).right_aligned()), chunks[1]);
}

fn tabs(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    let pending = app.pending().len();
    let agents_running = crate::agents::running_count(app.agents.as_ref());
    for (i, view) in View::ALL.iter().enumerate() {
        let label = if *view == View::Approvals && pending > 0 {
            format!(" {} {}({}) ", i + 1, view.label(), pending)
        } else if *view == View::Agents && agents_running > 0 {
            format!(" {} {}({}) ", i + 1, view.label(), agents_running)
        } else {
            format!(" {} {} ", i + 1, view.label())
        };
        spans.push(Span::styled(
            label,
            if *view == app.view {
                bold(TEXT).bg(SELECT_BG)
            } else if *view == View::Approvals && pending > 0 {
                Style::default().fg(AMBER)
            } else {
                muted()
            },
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(" Tab / F1-F6 で切替", muted()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── chat ──────────────────────────────────────────────────────────────────

fn meta_text(meta: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(ms) = meta["latency_ms"].as_u64() {
        parts.push(format!("{:.1}s", ms as f64 / 1000.0));
    }
    // Route + cost basis of this turn (tier/model, billing class, USD).
    // Older history entries only carry `tier`; the flat field stays as a
    // fallback so those rows still show where they went.
    if let Some(route) = meta.get("route").filter(|r| !r.is_null()) {
        if let Some(target) = kamimusuhi_resident::status::route_target_label(route) {
            parts.push(target);
        }
        if let Some(cost) = kamimusuhi_resident::status::route_cost_label(route) {
            parts.push(cost);
        }
        if let Some(cached) = route["cached_tokens"].as_u64().filter(|c| *c > 0) {
            parts.push(format!("cache {cached}tok"));
        }
    } else if let Some(tier) = meta["tier"].as_str() {
        parts.push(format!("via {tier}"));
    }
    let lookups = meta["reference_lookups"].as_u64().unwrap_or(0);
    if lookups > 0 {
        parts.push(format!("参照 {lookups}"));
    }
    let tools = meta["tool_calls"].as_array().cloned().unwrap_or_default();
    if !tools.is_empty() {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for t in &tools {
            let name = t["name"]
                .as_str()
                .unwrap_or("?")
                .trim_start_matches("mcp__")
                .replace("__", "/");
            if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == name) {
                entry.1 += 1;
            } else {
                counts.push((name, 1));
            }
        }
        for (name, n) in counts {
            parts.push(if n > 1 {
                format!("🔧{name}×{n}")
            } else {
                format!("🔧{name}")
            });
        }
    }
    parts.join(" · ")
}

fn message_lines(
    prefix: &str,
    style: Style,
    text: &str,
    tail: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    let mut it = text.split('\n');
    let mut first = vec![Span::styled(prefix.to_owned(), style)];
    first.push(Span::styled(
        it.next().unwrap_or("").to_owned(),
        Style::default().fg(TEXT),
    ));
    first.extend(tail);
    let mut lines = vec![Line::from(first)];
    for l in it {
        lines.push(Line::styled(format!("    {l}"), Style::default().fg(TEXT)));
    }
    lines
}

fn entry_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if app.entries.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("澪と話す", bold(TEXT)).centered());
        lines.push(
            Line::styled("Pi の常駐個体が記憶と連続性を持って応答します。", muted()).centered(),
        );
        return lines;
    }
    for entry in &app.entries {
        match entry {
            Entry::User { text, at } => {
                let tail: Vec<Span<'static>> = at
                    .as_deref()
                    .map(|ts| {
                        vec![Span::styled(
                            format!("  {}", ts.replace('T', " ").trim_end_matches('Z')),
                            muted(),
                        )]
                    })
                    .unwrap_or_default();
                lines.extend(message_lines(
                    "あなた> ",
                    bold(Color::Rgb(120, 180, 255)),
                    text,
                    tail,
                ));
            }
            Entry::Assistant { text, meta } => {
                lines.extend(message_lines("澪> ", bold(GREEN), text, Vec::new()));
                if let Some(meta) = meta {
                    let info = meta_text(meta);
                    if !info.is_empty() {
                        lines.push(Line::styled(format!("    {info}"), muted()));
                    }
                }
            }
            Entry::Notice { text, color } => {
                lines.push(Line::styled(text.clone(), Style::default().fg(*color)).centered());
            }
        }
        lines.push(Line::raw(""));
    }
    let pending = app.pending().len();
    if pending > 0 {
        lines.push(Line::styled(
            format!("⚠ 承認待ち {pending} 件 — F3 で承認ビューへ"),
            bold(AMBER),
        ));
    }
    if let Some(t) = app.busy_since {
        const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spinner = SPINNER[(t.elapsed().as_millis() / 120) as usize % SPINNER.len()];
        lines.push(Line::styled(
            format!(
                "{spinner} 考えています… {}s（参照・ツール使用時は 1〜2 分かかることがあります）",
                t.elapsed().as_secs()
            ),
            muted(),
        ));
    }
    lines
}

fn chat(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(4)]).split(area);
    let para = Paragraph::new(entry_lines(app)).wrap(Wrap { trim: false });
    let count = para.line_count(chunks[0].width);
    let max = count.saturating_sub(chunks[0].height as usize) as u16;
    app.chat_scroll_max = max;
    let scroll = if app.chat_follow {
        max
    } else {
        app.chat_scroll.min(max)
    };
    frame.render_widget(para.scroll((scroll, 0)), chunks[0]);

    let title = if app.url.is_none() {
        "接続中…".to_owned()
    } else if app.busy_since.is_some() {
        "考え中…".to_owned()
    } else {
        "メッセージ (Enter=送信 Alt+Enter/Ctrl+N=改行)".to_owned()
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL_RAISED));
    let inner = block.inner(chunks[1]);
    frame.render_widget(block, chunks[1]);

    let before = &app.input[..app.cursor.min(app.input.len())];
    let row = before.matches('\n').count() as u16;
    let col_w = UnicodeWidthStr::width(before.rsplit('\n').next().unwrap_or("")) as u16;
    let inner_w = inner.width.max(1);
    let inner_h = inner.height.max(1);
    let hscroll = col_w.saturating_sub(inner_w.saturating_sub(1));
    let vscroll = row.saturating_sub(inner_h.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .scroll((vscroll, hscroll))
            .style(Style::default().fg(TEXT)),
        inner,
    );
    frame.set_cursor_position((
        inner.x + (col_w - hscroll).min(inner_w.saturating_sub(1)),
        inner.y + (row - vscroll).min(inner_h.saturating_sub(1)),
    ));
}

// ── tasks ─────────────────────────────────────────────────────────────────

const LANES: [(&str, &str, Color); 5] = [
    ("in_progress", "進行中", BLUE),
    ("awaiting_operator", "あなたの判断待ち", AMBER),
    ("waiting", "待機", VIOLET),
    ("on_hold", "保留", MUTED),
    ("done", "完了", GREEN),
];

fn lane_of(task: &Value) -> &str {
    match task["status"].as_str().unwrap_or("") {
        "failed" | "cancelled" => "done",
        other => other,
    }
}

fn kind_glyph(kind: &str) -> &'static str {
    match kind {
        "dialogue" => "話",
        "tool" => "具",
        "approval" => "承",
        "commit" => "反",
        "job" => "定",
        "manual" => "手",
        _ => "澪",
    }
}

fn tasks(frame: &mut Frame, area: Rect, app: &mut App) {
    // Owned copies keep the borrow checker out of the scroll/selection updates.
    let visible: Vec<Value> = app.visible_tasks().into_iter().cloned().collect();
    if app.task_sel >= visible.len() {
        app.task_sel = visible.len().saturating_sub(1);
    }
    let selected = visible
        .get(app.task_sel)
        .and_then(|t| t["id"].as_str())
        .map(str::to_owned);
    let detail_w = if selected.is_some() { 42 } else { 0 };
    let chunks =
        Layout::horizontal([Constraint::Min(40), Constraint::Length(detail_w)]).split(area);

    let running = app
        .tasks
        .iter()
        .filter(|t| t["status"] == "in_progress")
        .count();
    let list_block = Block::bordered()
        .title(format!(
            "タスク — 進行中 {running} / 全 {}",
            app.tasks.len()
        ))
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL));
    let list_area = chunks[0];
    let list_inner = list_block.inner(list_area);
    frame.render_widget(list_block, list_area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line: Option<u16> = None;
    let mut idx = 0usize;
    let width = list_inner.width as usize;
    for (key, label, color) in LANES {
        let lane: Vec<&Value> = visible.iter().filter(|t| lane_of(t) == key).collect();
        if lane.is_empty() {
            continue;
        }
        lines.push(Line::styled(
            format!("─ {label} ({})", lane.len()),
            bold(color),
        ));
        for t in lane {
            let is_sel = idx == app.task_sel;
            if is_sel {
                sel_line = Some(lines.len() as u16);
            }
            let id = t["id"].as_str().unwrap_or("");
            let node = t["node"].as_str().unwrap_or("");
            let title = t["title"].as_str().unwrap_or("");
            let line = Line::from(vec![
                Span::styled(if is_sel { "▸ " } else { "  " }, Style::default().fg(BLUE)),
                Span::styled(
                    kind_glyph(t["kind"].as_str().unwrap_or("")).to_owned(),
                    bold(color),
                ),
                Span::styled(format!(" {title}"), Style::default().fg(TEXT)),
                Span::styled(format!("  {id} @{node}"), muted()),
            ]);
            let line = pad(line, width);
            lines.push(if is_sel {
                line.style(Style::default().bg(SELECT_BG))
            } else {
                line
            });
            idx += 1;
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled("タスクはありません。n で追加。", muted()));
    }
    let height = list_inner.height;
    app.tasks_scroll = clamp_scroll(lines.len(), height, app.tasks_scroll);
    if let Some(sel) = sel_line {
        if sel < app.tasks_scroll {
            app.tasks_scroll = sel;
        } else if sel >= app.tasks_scroll + height {
            app.tasks_scroll = sel - height + 1;
        }
    }
    frame.render_widget(
        Paragraph::new(lines).scroll((app.tasks_scroll, 0)),
        list_inner,
    );

    if let Some(id) = selected {
        let detail_block = Block::bordered()
            .title("詳細")
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(PANEL));
        let detail_inner = detail_block.inner(chunks[1]);
        frame.render_widget(detail_block, chunks[1]);
        let Some(t) = app
            .tasks
            .iter()
            .find(|t| t["id"].as_str() == Some(id.as_str()))
        else {
            return;
        };
        let mut lines = vec![
            Line::styled(t["title"].as_str().unwrap_or("").to_owned(), bold(TEXT)),
            Line::styled(
                format!(
                    "{} · {} · {}",
                    s(t.get("id")),
                    s(t.get("status")),
                    s(t.get("kind"))
                ),
                muted(),
            ),
            Line::styled(
                format!("@{} by {}", s(t.get("node")), s(t.get("owner"))),
                muted(),
            ),
            Line::styled(format!("作成 {}", s(t.get("created"))), muted()),
            Line::styled(format!("更新 {}", s(t.get("updated"))), muted()),
        ];
        if let Some(d) = t["duration_secs"].as_u64() {
            lines.push(Line::styled(format!("所要 {d}s"), muted()));
        }
        let deps: Vec<String> = t["depends_on"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str().map(str::to_owned))
            .collect();
        if !deps.is_empty() {
            lines.push(Line::styled(format!("前提: {}", deps.join(", ")), muted()));
        }
        let children: Vec<String> = app
            .tasks
            .iter()
            .filter(|c| {
                c["depends_on"]
                    .as_array()
                    .is_some_and(|d| d.iter().any(|x| x.as_str() == Some(id.as_str())))
            })
            .filter_map(|c| c["id"].as_str().map(str::to_owned))
            .collect();
        if !children.is_empty() {
            lines.push(Line::styled(
                format!("後続: {}", children.join(", ")),
                muted(),
            ));
        }
        for note in t["notes"].as_array().into_iter().flatten().rev().take(5) {
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!("{}:", s(note.get("by"))), bold(MUTED)));
            for l in s(note.get("text")).lines() {
                lines.push(Line::styled(format!("  {l}"), Style::default().fg(TEXT)));
            }
        }
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            detail_inner,
        );
    }
}

// ── approvals ─────────────────────────────────────────────────────────────

/// One line for an approval: tool name + the most telling argument.
pub fn approval_title(a: &Value) -> String {
    let args = &a["arguments"];
    let target = ["path", "file", "note", "oldPath", "url"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .unwrap_or("");
    format!(
        "{} {}",
        a["exposed"]
            .as_str()
            .unwrap_or("?")
            .trim_start_matches("mcp__")
            .replace("__", "/"),
        target
    )
}

fn approvals(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Percentage(45), Constraint::Min(8)]).split(area);
    let pending: Vec<Value> = app.pending().into_iter().cloned().collect();
    if app.approval_sel >= pending.len() {
        app.approval_sel = pending.len().saturating_sub(1);
    }

    let list_block = Block::bordered()
        .title(format!("承認待ち {}", pending.len()))
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL));
    let list_inner = list_block.inner(chunks[0]);
    frame.render_widget(list_block, chunks[0]);
    let width = list_inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if pending.is_empty() {
        lines.push(Line::styled("承認待ちはありません。", muted()));
    }
    for (i, a) in pending.iter().enumerate() {
        let is_sel = i == app.approval_sel;
        let id = a["id"].as_str().unwrap_or("");
        let deciding = app.deciding.contains(id);
        let line = Line::from(vec![
            Span::styled(if is_sel { "▸ " } else { "  " }, Style::default().fg(AMBER)),
            Span::styled("✋ ", Style::default().fg(AMBER)),
            Span::styled(approval_title(a), bold(TEXT)),
            Span::styled(format!("  {} {}", id, s(a.get("created"))), muted()),
            Span::styled(if deciding { "  実行中…" } else { "" }, muted()),
        ]);
        let line = pad(line, width);
        lines.push(if is_sel {
            line.style(Style::default().bg(SELECT_BG))
        } else {
            line
        });
    }
    let recent: Vec<&Value> = app
        .approvals
        .iter()
        .filter(|a| a["state"] != "pending")
        .take(8)
        .collect();
    if !recent.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("── 最近の決定 ──", muted()));
        for a in recent {
            let color = match a["state"].as_str() {
                Some("done") => GREEN,
                Some("failed") => RED,
                _ => MUTED,
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<9}", a["state"].as_str().unwrap_or("?")),
                    bold(color),
                ),
                Span::styled(approval_title(a), Style::default().fg(TEXT)),
                Span::styled(format!("  {}", s(a.get("decided"))), muted()),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), list_inner);

    let detail_block = Block::bordered()
        .title("内容 — a: 承認して実行 / x: 却下 / J,K: スクロール")
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL));
    let detail_inner = detail_block.inner(chunks[1]);
    frame.render_widget(detail_block, chunks[1]);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(a) = pending.get(app.approval_sel) {
        lines.push(Line::styled(approval_title(a), bold(AMBER)));
        lines.push(Line::styled(
            format!("id {} · {}", s(a.get("id")), s(a.get("created"))),
            muted(),
        ));
        let args = &a["arguments"];
        if let Some(content) = ["content", "newString", "text"]
            .iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
        {
            lines.push(Line::raw(""));
            for l in content.lines() {
                lines.push(Line::styled(l.to_owned(), Style::default().fg(TEXT)));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("引数:", muted()));
        for l in serde_json::to_string_pretty(args)
            .unwrap_or_default()
            .lines()
        {
            lines.push(Line::styled(format!("  {l}"), muted()));
        }
    }
    app.detail_scroll = clamp_scroll(lines.len(), detail_inner.height, app.detail_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((app.detail_scroll, 0)),
        detail_inner,
    );
}

// ── status ────────────────────────────────────────────────────────────────

fn status(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let ago = app
        .status_at
        .map(|t| format!("{}秒前に更新", t.elapsed().as_secs()))
        .unwrap_or_else(|| "未取得".to_owned());
    lines.push(Line::styled(format!("状態 — {ago} · r: 再取得"), muted()));
    lines.push(Line::raw(""));
    match &app.status_text {
        Some(text) => {
            for l in text.lines() {
                lines.push(Line::styled(l.to_owned(), Style::default().fg(TEXT)));
            }
        }
        None => lines.push(Line::styled("読み込み中…", muted())),
    }
    app.status_scroll = clamp_scroll(lines.len(), area.height, app.status_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((app.status_scroll, 0)), area);
}

// ── tools ─────────────────────────────────────────────────────────────────

fn tools(frame: &mut Frame, area: Rect, app: &mut App) {
    let filter = app.tool_filter.to_lowercase();
    let title = if filter.is_empty() {
        "ツール — / で絞り込み".to_owned()
    } else {
        format!("ツール — 絞り込み: {}", app.tool_filter)
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines: Vec<Line<'static>> = Vec::new();
    match &app.tools {
        None => lines.push(Line::styled("読み込み中…", muted())),
        Some(tools) => {
            lines.push(Line::styled("ライブラリ", bold(TEXT)));
            for l in tools["libraries"].as_array().into_iter().flatten() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} @{}", s(l.get("name")), s(l.get("node"))),
                        bold(TEXT),
                    ),
                    Span::styled(format!("  {}", s(l.get("description"))), muted()),
                ]));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled("MCP サーバー", bold(TEXT)));
            for m in tools["mcp_servers"].as_array().into_iter().flatten() {
                let ok = m["state"] == "running";
                lines.push(Line::from(vec![
                    Span::styled("  ● ", Style::default().fg(if ok { GREEN } else { RED })),
                    Span::styled(
                        format!("{} @{}", s(m.get("name")), s(m.get("node"))),
                        bold(TEXT),
                    ),
                    Span::styled(
                        format!(
                            "  {} tools · {}",
                            s(m.get("tools")),
                            s(m.get("description"))
                        ),
                        muted(),
                    ),
                ]));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled("tool 一覧", bold(TEXT)));
            for t in tools["tools"].as_array().into_iter().flatten() {
                let f = &t["function"];
                let name = f["name"].as_str().unwrap_or("");
                let desc = f["description"].as_str().unwrap_or("");
                if !filter.is_empty()
                    && !name.to_lowercase().contains(&filter)
                    && !desc.to_lowercase().contains(&filter)
                {
                    continue;
                }
                let gated = desc.contains("要承認");
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {name}"),
                        Style::default().fg(if gated { AMBER } else { VIOLET }),
                    ),
                    Span::styled(
                        format!("  {}", desc.chars().take(120).collect::<String>()),
                        muted(),
                    ),
                ]));
            }
        }
    }
    app.tools_scroll = clamp_scroll(lines.len(), inner.height, app.tools_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((app.tools_scroll, 0)), inner);
}

// ── footer ────────────────────────────────────────────────────────────────

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(error) = &app.error {
        frame.render_widget(
            Paragraph::new(Line::styled(format!("⚠ {error}"), Style::default().fg(RED))),
            area,
        );
        return;
    }
    match app.mode {
        InputMode::NewTask { depends } => {
            let prompt = if depends {
                "新しいタスク（選択の後続）> "
            } else {
                "新しいタスク> "
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prompt, bold(BLUE)),
                    Span::styled(app.input.clone(), Style::default().fg(TEXT)),
                    Span::styled("   Enter: 作成 / Esc: やめる", muted()),
                ])),
                area,
            );
            let x = area.x
                + (UnicodeWidthStr::width(prompt) + UnicodeWidthStr::width(app.input.as_str()))
                    as u16;
            frame.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y));
        }
        InputMode::ToolFilter => {
            let prompt = "絞り込み> ";
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prompt, bold(VIOLET)),
                    Span::styled(app.tool_filter.clone(), Style::default().fg(TEXT)),
                    Span::styled("   Enter/Esc: 確定", muted()),
                ])),
                area,
            );
            let x = area.x
                + (UnicodeWidthStr::width(prompt)
                    + UnicodeWidthStr::width(
                        &app.tool_filter[..app.tool_cursor.min(app.tool_filter.len())],
                    )) as u16;
            frame.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y));
        }
        InputMode::AgentContinue { ref id } => {
            let prompt = format!("続けて依頼（{id}）> ");
            let x = area.x
                + (UnicodeWidthStr::width(prompt.as_str())
                    + UnicodeWidthStr::width(
                        &app.agent_input[..app.agent_cursor.min(app.agent_input.len())],
                    )) as u16;
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prompt, bold(BLUE)),
                    Span::styled(app.agent_input.clone(), Style::default().fg(TEXT)),
                    Span::styled("   Enter: 送信 / Esc: やめる", muted()),
                ])),
                area,
            );
            frame.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y));
        }
        InputMode::Normal => {
            let hint = match app.view {
                View::Chat => {
                    "Enter: 送信 · Alt+Enter/Ctrl+N: 改行 · ↑↓/PgUp/PgDn: 履歴 · Tab: ビュー切替 · Ctrl+C: 終了"
                }
                View::Tasks => {
                    "j/k: 選択 · n: 新規 · N: 選択の後続 · d: 完了にする · h: 完了表示 · r: 更新 · q: 終了"
                }
                View::Approvals => {
                    "j/k: 選択 · a: 承認して実行 · x: 却下 · J/K: 詳細スクロール · r: 更新 · q: 終了"
                }
                View::Status => "j/k/PgUp/PgDn: スクロール · r: 更新 · q: 終了",
                View::Tools => "/: 絞り込み · c: クリア · j/k: スクロール · r: 更新 · q: 終了",
                View::Agents => {
                    "↑↓/j/k: 選択 · PgUp/PgDn: 詳細 · c: 取消 · a/r: 採用/不採用 · n: 続けて依頼 · A/D: worktree 適用/破棄 · h: 実行中のみ · R: 再検査 · q: 終了"
                }
            };
            frame.render_widget(Paragraph::new(Line::styled(hint, muted())), area);
        }
    }
}
