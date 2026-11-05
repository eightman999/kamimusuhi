//! Task Plane view: external agent executors, their tasks, the usage ledger
//! and per-model statistics, as served by the resident's `/v1/agents`.
//!
//! Every field of the panel document is optional on the wire; missing values
//! render as "—" and unknown costs as "不明" (never as zero). Mirrors the
//! desktop `task_panel.rs`; the helpers are reimplemented here on purpose.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, Wrap};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::ui::{
    AMBER, BLUE, BORDER, GREEN, MUTED, PANEL, RED, SELECT_BG, TEXT, VIOLET, bold, clamp_scroll,
    muted, pad,
};

const DASH: &str = "—";
const UNKNOWN: &str = "不明";
/// Tool activity entries shown at the end of the detail.
const ACTIVITY_TAIL: usize = 10;

/// Detail keys shown first, in this order; any other key follows.
const DETAIL_KEYS: [(&str, &str); 15] = [
    ("executor", "実行者"),
    ("model", "モデル"),
    ("model_ref", "モデル参照"),
    ("billing", "課金"),
    ("task_kind", "種類"),
    ("workspace", "作業場所"),
    ("permissions", "権限"),
    ("routing", "経路選択"),
    ("shadow_routing", "影の経路"),
    ("external_session_id", "外部セッション"),
    ("tool_calls", "ツール呼出"),
    ("continues", "継続元"),
    ("eval", "評価"),
    ("worktree", "worktree"),
    ("branch", "ブランチ"),
];

// ── state helpers (used by app.rs) ─────────────────────────────────────────

/// Tasks of the panel, newest first; `running_only` keeps waiting/in_progress.
pub fn filtered_tasks(panel: Option<&Value>, running_only: bool) -> Vec<&Value> {
    let mut tasks: Vec<&Value> = panel
        .and_then(|p| p["tasks"].as_array())
        .into_iter()
        .flatten()
        .filter(|t| !running_only || is_active(t))
        .collect();
    // Stable sort: equal keys keep server order.
    tasks.sort_by(|a, b| sort_key(b).cmp(sort_key(a)));
    tasks
}

/// Index to select after the list changed: follow `prev_id` when it is still
/// listed, otherwise clamp `prev_idx` into range. `None` for an empty list.
pub fn resolve_selection(ids: &[&str], prev_id: Option<&str>, prev_idx: usize) -> Option<usize> {
    if ids.is_empty() {
        return None;
    }
    prev_id
        .and_then(|id| ids.iter().position(|x| *x == id))
        .or(Some(prev_idx.min(ids.len() - 1)))
}

pub fn is_active(t: &Value) -> bool {
    matches!(t["status"].as_str(), Some("waiting" | "in_progress"))
}

pub fn is_finished(t: &Value) -> bool {
    matches!(t["status"].as_str(), Some("done" | "failed" | "cancelled"))
}

/// A finished task with an external session can be continued.
pub fn can_continue(t: &Value) -> bool {
    is_finished(t)
        && t["result"]["session_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
}

/// A finished write task with an isolated worktree can be applied/discarded.
pub fn has_worktree(t: &Value) -> bool {
    is_finished(t) && !t["detail"]["worktree"].is_null()
}

/// Short text for an action reply.
pub fn reply_text(reply: &Value) -> String {
    let text = reply["message"]
        .as_str()
        .or_else(|| reply["error"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| reply.to_string());
    clip(&text, 300)
}

pub fn running_count(panel: Option<&Value>) -> usize {
    panel
        .and_then(|p| p["tasks"].as_array())
        .into_iter()
        .flatten()
        .filter(|t| t["status"] == "in_progress")
        .count()
}

// ── pure formatting ───────────────────────────────────────────────────────

fn sort_key(t: &Value) -> &str {
    t["created"]
        .as_str()
        .or_else(|| t["updated"].as_str())
        .unwrap_or("")
}

pub fn status_label(status: &str) -> &str {
    match status {
        "waiting" => "待機",
        "in_progress" => "実行中",
        "done" => "完了",
        "failed" => "失敗",
        "cancelled" => "取消",
        "" => DASH,
        other => other,
    }
}

pub fn status_color(status: &str) -> Color {
    match status {
        "waiting" => VIOLET,
        "in_progress" => BLUE,
        "done" => GREEN,
        "failed" => RED,
        _ => MUTED,
    }
}

pub fn executor_state(e: &Value) -> (&'static str, Color) {
    if e["enabled"].as_bool() == Some(false) {
        ("無効", MUTED)
    } else if e["ok"].as_bool() == Some(true) {
        ("OK", GREEN)
    } else {
        ("未検出", RED)
    }
}

pub fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(max_chars).collect();
        s.push('…');
        s
    }
}

/// Any JSON value as display text; null / missing / empty → "—".
pub fn text_or_dash(v: &Value) -> String {
    match v {
        Value::Null => DASH.to_owned(),
        Value::String(s) if s.is_empty() => DASH.to_owned(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "はい" } else { "いいえ" }.to_owned(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

pub fn num_or_dash(v: &Value) -> String {
    if v.is_number() {
        v.to_string()
    } else {
        DASH.to_owned()
    }
}

fn list_items(v: &Value) -> Vec<String> {
    match v.as_array() {
        Some(a) => a.iter().map(text_or_dash).collect(),
        None if v.is_null() => Vec::new(),
        None => vec![text_or_dash(v)],
    }
}

fn list_or_dash(v: &Value) -> String {
    let items = list_items(v);
    if items.is_empty() {
        DASH.to_owned()
    } else {
        items.join(", ")
    }
}

fn timestamp(v: &Value) -> String {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map_or_else(|| DASH.to_owned(), |s| s.replace('T', " "))
}

/// Dollar amount; a missing / null price is unknown, never zero.
pub fn fmt_usd(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x == 0.0 || x.abs() >= 0.01 => format!("${x:.2}"),
        Some(x) => format!("${x:.4}"),
        None => UNKNOWN.to_owned(),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

pub fn fmt_usage(usage: &Value) -> String {
    let part = |key: &str| {
        usage[key]
            .as_u64()
            .map_or_else(|| DASH.to_owned(), fmt_tokens)
    };
    format!(
        "入力 {} · 出力 {} · cache読 {} · cache書 {}",
        part("input_tokens"),
        part("output_tokens"),
        part("cache_read_tokens"),
        part("cache_write_tokens"),
    )
}

pub fn fmt_duration(secs: Option<f64>) -> String {
    let Some(secs) = secs.filter(|s| s.is_finite() && *s >= 0.0) else {
        return DASH.to_owned();
    };
    let s = secs.round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

fn fmt_success(succeeded: &Value, n: &Value) -> String {
    match (succeeded.as_u64(), n.as_u64()) {
        (Some(ok), Some(n)) if n > 0 => {
            format!("{ok} ({:.0}%)", ok as f64 * 100.0 / n as f64)
        }
        (Some(ok), _) => ok.to_string(),
        _ => DASH.to_owned(),
    }
}

pub fn fmt_count_quota(used: &Value, max: &Value) -> String {
    let used = used.as_u64().unwrap_or(0);
    match max.as_u64() {
        Some(max) => format!("{used}/{max} 件"),
        None => format!("{used} 件"),
    }
}

pub fn fmt_usd_quota(used: &Value, unpriced: &Value, max: &Value) -> String {
    // No entry at all: nothing was spent (zero tasks), not an unknown price.
    let mut out = if used.is_null() {
        "$0.00".to_owned()
    } else {
        fmt_usd(used)
    };
    if max.is_number() {
        out = format!("{out}/{}", fmt_usd(max));
    }
    if let Some(n) = unpriced.as_u64().filter(|n| *n > 0) {
        out.push_str(&format!(" (+{n} 件{UNKNOWN})"));
    }
    out
}

pub fn fmt_tokens_quota(used: &Value, max: &Value) -> String {
    let used = fmt_tokens(used.as_u64().unwrap_or(0));
    match max.as_u64() {
        Some(max) => format!("{used}/{}", fmt_tokens(max)),
        None => used,
    }
}

/// Sum of per-executor ledger entries (for the total row).
fn sum_usage(per_executor: &Value) -> Value {
    let (mut tasks, mut usd, mut tokens, mut unpriced) = (0u64, 0f64, 0u64, 0u64);
    for (_, e) in per_executor.as_object().into_iter().flatten() {
        tasks += e["tasks"].as_u64().unwrap_or(0);
        usd += e["usd"].as_f64().unwrap_or(0.0);
        tokens += e["tokens"].as_u64().unwrap_or(0);
        unpriced += e["unpriced"].as_u64().unwrap_or(0);
    }
    json!({"tasks": tasks, "usd": usd, "tokens": tokens, "unpriced": unpriced})
}

fn models_count(models: &Value) -> String {
    match models {
        Value::Array(a) => a.len().to_string(),
        Value::Object(o) => o.len().to_string(),
        other => num_or_dash(other),
    }
}

// ── drawing ───────────────────────────────────────────────────────────────

fn bordered(title: String) -> Block<'static> {
    Block::bordered()
        .title(title)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL))
}

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.agents_absent {
        let block = bordered("外部エージェント".to_owned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::styled("この node には Task Plane がありません", muted()).centered(),
            ]),
            inner,
        );
        return;
    }
    let Some(panel) = app.agents.clone() else {
        let block = bordered("外部エージェント".to_owned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(Line::styled("読み込み中…", muted())), inner);
        return;
    };

    let executors: Vec<&Value> = panel["health"]["executors"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let config_error = panel["health"]["config_error"]
        .as_str()
        .filter(|e| !e.is_empty());
    let strip_lines = executors.len().clamp(1, 4)
        + usize::from(config_error.is_some())
        + usize::from(app.agent_reply.is_some());
    let rows = Layout::vertical([
        Constraint::Length(strip_lines as u16 + 2),
        Constraint::Min(6),
        Constraint::Length(9),
    ])
    .split(area);

    executor_strip(frame, rows[0], app, &panel, &executors, config_error);

    let tasks = filtered_tasks(Some(&panel), app.agents_running_only);
    let ids: Vec<&str> = tasks
        .iter()
        .map(|t| t["id"].as_str().unwrap_or(""))
        .collect();
    let sel = resolve_selection(&ids, app.agent_sel_id.as_deref(), app.agent_sel);
    app.agent_sel = sel.unwrap_or(0);
    app.agent_sel_id = sel.map(|i| ids[i].to_owned());
    let selected = sel.map(|i| tasks[i]);

    let main =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(rows[1]);
    task_list(frame, main[0], app, &tasks, sel, panel["tasks"].as_array());
    task_detail(frame, main[1], app, selected);

    let bottom =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[2]);
    ledger(frame, bottom[0], &panel["ledger"]);
    stats(frame, bottom[1], &panel["stats"]);
}

fn executor_strip(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    panel: &Value,
    executors: &[&Value],
    config_error: Option<&str>,
) {
    let h = &panel["health"];
    let block = bordered(format!(
        "外部エージェント — 実行中 {}/{} · 待機含む {} · 経路 {}",
        num_or_dash(&h["running"]),
        num_or_dash(&h["max_concurrent"]),
        num_or_dash(&h["queued_or_running"]),
        text_or_dash(&h["routing_mode"]),
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(e) = config_error {
        lines.push(Line::styled(format!("設定エラー: {e}"), bold(RED)));
    }
    if executors.is_empty() {
        lines.push(Line::styled("実行者が設定されていません", muted()));
    }
    for e in executors {
        let (state, color) = executor_state(e);
        let mut spans = vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(text_or_dash(&e["name"]), bold(TEXT)),
            Span::styled(format!(" {state}"), Style::default().fg(color)),
            Span::styled(
                format!(
                    "  {} · 実行 {}/{} · モデル {} (既定 {})",
                    text_or_dash(&e["protocol"]),
                    num_or_dash(&e["running"]),
                    num_or_dash(&e["max_concurrent"]),
                    models_count(&e["models"]),
                    text_or_dash(&e["default_model"]),
                ),
                muted(),
            ),
        ];
        let sup = &e["supervisor"];
        if sup.is_object() {
            let st = sup["state"].as_str().unwrap_or(DASH);
            let sup_color = match st {
                "running" => GREEN,
                "starting" => AMBER,
                "failed" => RED,
                _ => MUTED,
            };
            let conns = sup["connections"]
                .as_u64()
                .map(|n| format!(" 接続 {n}"))
                .unwrap_or_default();
            spans.push(Span::styled(
                format!(" · 監督 {} {st}{conns}", text_or_dash(&sup["kind"])),
                Style::default().fg(sup_color),
            ));
        }
        if let Some(msg) = e["catalog_error"].as_str().filter(|s| !s.is_empty()) {
            spans.push(Span::styled(
                format!(" · カタログ: {}", clip(msg, 60)),
                Style::default().fg(RED),
            ));
        } else if let Some(msg) = e["detail"].as_str().filter(|s| !s.is_empty()) {
            spans.push(Span::styled(format!(" · {}", clip(msg, 60)), muted()));
        }
        lines.push(Line::from(spans));
    }
    if let Some(r) = &app.agent_reply {
        lines.push(Line::styled(format!("直近の操作: {r}"), muted()));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn task_list(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    tasks: &[&Value],
    sel: Option<usize>,
    all: Option<&Vec<Value>>,
) {
    let filter = if app.agents_running_only {
        "実行中のみ"
    } else {
        "すべて"
    };
    let block = bordered(format!(
        "タスク {}/{} — {filter}",
        tasks.len(),
        all.map_or(0, Vec::len)
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if tasks.is_empty() {
        lines.push(Line::styled("表示するタスクはありません", muted()));
    }
    for (i, t) in tasks.iter().enumerate() {
        let is_sel = sel == Some(i);
        let status = t["status"].as_str().unwrap_or("");
        let color = status_color(status);
        let d = &t["detail"];
        let mut first = vec![
            Span::styled(if is_sel { "▸ " } else { "  " }, Style::default().fg(BLUE)),
            Span::styled(format!("{} ", status_label(status)), bold(color)),
            Span::styled(
                t["title"].as_str().unwrap_or(DASH).to_owned(),
                Style::default().fg(TEXT),
            ),
        ];
        if t["result"]["read_only_violation"].as_bool() == Some(true) {
            first.push(Span::styled(" ⚠", bold(RED)));
        }
        let second = Line::styled(
            format!(
                "    {}·{} · {}",
                text_or_dash(&d["executor"]),
                text_or_dash(&d["model"]),
                fmt_duration(t["duration_secs"].as_f64()),
            ),
            muted(),
        );
        for line in [Line::from(first), second] {
            let line = pad(line, width);
            lines.push(if is_sel {
                line.style(Style::default().bg(SELECT_BG))
            } else {
                line
            });
        }
    }
    // Two lines per task: keep the selection visible.
    let height = inner.height;
    app.agent_list_scroll = clamp_scroll(lines.len(), height, app.agent_list_scroll);
    if let Some(i) = sel {
        let top = (i * 2) as u16;
        if top < app.agent_list_scroll {
            app.agent_list_scroll = top;
        } else if top + 2 > app.agent_list_scroll + height {
            app.agent_list_scroll = (top + 2).saturating_sub(height);
        }
    }
    frame.render_widget(
        Paragraph::new(lines).scroll((app.agent_list_scroll, 0)),
        inner,
    );
}

fn section(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::raw(""));
    lines.push(Line::styled(format!("── {title}"), bold(MUTED)));
}

fn kv(lines: &mut Vec<Line<'static>>, key: &str, value: String) {
    lines.push(Line::from(vec![
        Span::styled(format!("{key}: "), muted()),
        Span::styled(value, Style::default().fg(TEXT)),
    ]));
}

/// Detail lines of one task (pure; used by the renderer and tests).
pub fn detail_lines(t: &Value) -> Vec<Line<'static>> {
    let status = t["status"].as_str().unwrap_or("");
    let mut lines = vec![
        Line::styled(t["title"].as_str().unwrap_or(DASH).to_owned(), bold(TEXT)),
        Line::from(vec![
            Span::styled(status_label(status).to_owned(), bold(status_color(status))),
            Span::styled(format!("  {}", text_or_dash(&t["id"])), muted()),
        ]),
    ];
    let mut keys: Vec<&str> = Vec::new();
    if is_active(t) {
        keys.push("c: 取消");
    }
    if is_finished(t) {
        keys.push("a: 採用 · r: 不採用");
    }
    if can_continue(t) {
        keys.push("n: 続けて依頼");
    }
    if has_worktree(t) {
        keys.push("A: worktree 適用 · D: 破棄");
    }
    if !keys.is_empty() {
        lines.push(Line::styled(keys.join(" · "), Style::default().fg(BLUE)));
    }

    section(&mut lines, "概要");
    kv(&mut lines, "担当", text_or_dash(&t["owner"]));
    kv(&mut lines, "作成", timestamp(&t["created"]));
    kv(&mut lines, "更新", timestamp(&t["updated"]));
    kv(&mut lines, "終了", timestamp(&t["finished"]));
    kv(
        &mut lines,
        "所要",
        fmt_duration(t["duration_secs"].as_f64()),
    );
    kv(&mut lines, "依存", list_or_dash(&t["depends_on"]));
    let detail = &t["detail"];
    for (k, label) in DETAIL_KEYS {
        // Core keys always show (as "—" when missing); the rest only when set.
        let core = matches!(
            k,
            "executor" | "model" | "billing" | "task_kind" | "workspace" | "permissions"
        );
        if core || !detail[k].is_null() {
            kv(&mut lines, label, text_or_dash(&detail[k]));
        }
    }
    for (k, v) in detail.as_object().into_iter().flatten() {
        if !DETAIL_KEYS.iter().any(|(key, _)| key == k) {
            kv(&mut lines, k, text_or_dash(v));
        }
    }

    section(&mut lines, "経過メモ");
    let notes: Vec<&Value> = t["notes"].as_array().into_iter().flatten().collect();
    if notes.is_empty() {
        lines.push(Line::styled(DASH, muted()));
    }
    for n in notes.iter().rev() {
        lines.push(Line::styled(
            format!(
                "{} [{}]",
                timestamp(&n["at"]),
                n["by"].as_str().unwrap_or(DASH)
            ),
            muted(),
        ));
        for l in n["text"].as_str().unwrap_or("").lines() {
            lines.push(Line::styled(format!("  {l}"), Style::default().fg(TEXT)));
        }
    }

    section(&mut lines, "結果");
    let result = &t["result"];
    if !result.is_object() {
        lines.push(Line::styled("結果はまだありません", muted()));
        return lines;
    }
    if result["read_only_violation"].as_bool() == Some(true) {
        lines.push(Line::styled(
            "⚠ 読み取り専用のはずのタスクがファイルを変更しました",
            bold(RED),
        ));
    }
    if let Some(e) = result["error"].as_str().filter(|e| !e.is_empty()) {
        lines.push(Line::styled(
            format!("エラー: {e}"),
            Style::default().fg(RED),
        ));
    }
    let summary = result["summary"].as_str().unwrap_or("");
    if summary.is_empty() {
        lines.push(Line::styled(DASH, muted()));
    }
    for l in summary.lines() {
        lines.push(Line::styled(l.to_owned(), Style::default().fg(TEXT)));
    }
    lines.push(Line::raw(""));
    let cost = &result["cost"];
    kv(&mut lines, "使用量", fmt_usage(&result["usage"]));
    kv(
        &mut lines,
        "費用",
        format!(
            "報告 {} · 推定 {} · {}",
            fmt_usd(&cost["reported_usd"]),
            fmt_usd(&cost["estimated_usd"]),
            text_or_dash(&cost["billing"])
        ),
    );
    let files = list_items(&result["files_changed"]);
    kv(&mut lines, "変更ファイル", format!("{} 件", files.len()));
    for f in &files {
        lines.push(Line::styled(format!("  {f}"), Style::default().fg(TEXT)));
    }
    if !result["diff_stat"].is_null() {
        kv(&mut lines, "差分", text_or_dash(&result["diff_stat"]));
    }
    let activity = list_items(&result["tool_activity"]);
    kv(
        &mut lines,
        "ツール活動",
        if activity.is_empty() {
            DASH.to_owned()
        } else {
            format!("{} 件（末尾 {ACTIVITY_TAIL}）", activity.len())
        },
    );
    for a in activity
        .iter()
        .skip(activity.len().saturating_sub(ACTIVITY_TAIL))
    {
        lines.push(Line::styled(format!("  {}", clip(a, 200)), muted()));
    }
    kv(
        &mut lines,
        "セッション",
        text_or_dash(&result["session_id"]),
    );
    kv(&mut lines, "証跡", text_or_dash(&result["evidence_id"]));
    kv(&mut lines, "評価", text_or_dash(&result["eval"]));
    kv(
        &mut lines,
        "フィードバック",
        match result["feedback"].as_str() {
            Some("accept") => "採用".into(),
            Some("reject") => "不採用".into(),
            _ => text_or_dash(&result["feedback"]),
        },
    );
    lines
}

fn task_detail(frame: &mut Frame, area: Rect, app: &mut App, task: Option<&Value>) {
    let block = bordered("詳細 — PgUp/PgDn: スクロール".to_owned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(t) = task else {
        frame.render_widget(Paragraph::new(Line::styled(DASH, muted())), inner);
        return;
    };
    let para = Paragraph::new(detail_lines(t)).wrap(Wrap { trim: false });
    let count = para.line_count(inner.width);
    app.agent_detail_scroll = clamp_scroll(count, inner.height, app.agent_detail_scroll);
    frame.render_widget(para.scroll((app.agent_detail_scroll, 0)), inner);
}

fn header_row(cells: &[&'static str]) -> Row<'static> {
    Row::new(cells.iter().map(|c| Cell::from(*c))).style(bold(MUTED))
}

fn ledger(frame: &mut Frame, area: Rect, ledger: &Value) {
    let block = bordered(format!(
        "利用台帳 — 今日 {} · 今月 {}",
        text_or_dash(&ledger["day"]),
        text_or_dash(&ledger["month"])
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (today, month, quotas) = (&ledger["today"], &ledger["this_month"], &ledger["quotas"]);
    let mut names: Vec<String> = [today, month, quotas]
        .iter()
        .flat_map(|v| v.as_object().into_iter().flatten().map(|(k, _)| k.clone()))
        .filter(|k| k != "_total")
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() && quotas.get("_total").is_none() {
        frame.render_widget(
            Paragraph::new(Line::styled("記録はまだありません", muted())),
            inner,
        );
        return;
    }
    let total = quotas.get("_total").is_some() || names.len() > 1;
    let mut rows: Vec<(String, Value, Value)> = names
        .iter()
        .map(|n| (n.clone(), today[n].clone(), month[n].clone()))
        .collect();
    if total {
        rows.push((
            "_total".to_owned(),
            today
                .get("_total")
                .cloned()
                .unwrap_or_else(|| sum_usage(today)),
            month
                .get("_total")
                .cloned()
                .unwrap_or_else(|| sum_usage(month)),
        ));
    }
    let name_w = rows
        .iter()
        .map(|(n, _, _)| UnicodeWidthStr::width(n.as_str()))
        .max()
        .unwrap_or(0)
        .clamp(6, 16) as u16;
    // Two rows per executor (today / month) so quotas fit at half width.
    let rows = rows.into_iter().flat_map(|(name, d, m)| {
        let q = &quotas[name.as_str()];
        let label = if name == "_total" {
            "合計".to_owned()
        } else {
            name.clone()
        };
        let today = format!(
            "{} · {} · {} tok",
            fmt_count_quota(&d["tasks"], &q["max_tasks_per_day"]),
            fmt_usd_quota(&d["usd"], &d["unpriced"], &q["max_usd_per_day"]),
            fmt_tokens_quota(&d["tokens"], &q["max_tokens_per_day"]),
        );
        let month = format!(
            "{} · {}",
            fmt_count_quota(&m["tasks"], &q["max_tasks_per_month"]),
            fmt_usd_quota(&m["usd"], &m["unpriced"], &q["max_usd_per_month"]),
        );
        [
            Row::new(vec![
                Cell::from(label).style(bold(TEXT)),
                Cell::from("今日").style(muted()),
                Cell::from(today),
            ]),
            Row::new(vec![
                Cell::from(""),
                Cell::from("今月").style(muted()),
                Cell::from(month),
            ]),
        ]
        .map(|row| row.style(Style::default().fg(TEXT)))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(name_w),
            Constraint::Length(4),
            Constraint::Fill(1),
        ],
    )
    .header(header_row(&["実行者", "期間", "件数 · 費用 · tokens"]))
    .column_spacing(1);
    frame.render_widget(table, inner);
}

fn stats(frame: &mut Frame, area: Rect, stats: &Value) {
    let block = bordered("実績（モデル × ハーネス）".to_owned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let list: Vec<&Value> = stats.as_array().into_iter().flatten().collect();
    if list.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled("実績はまだありません", muted())),
            inner,
        );
        return;
    }
    let rows = list.into_iter().map(|s| {
        Row::new(vec![
            Cell::from(text_or_dash(&s["task_kind"])),
            Cell::from(text_or_dash(&s["executor"])),
            Cell::from(text_or_dash(&s["model"])),
            Cell::from(num_or_dash(&s["n"])),
            Cell::from(fmt_success(&s["succeeded"], &s["n"])),
            Cell::from(format!(
                "{}/{}",
                num_or_dash(&s["accepted"]),
                num_or_dash(&s["rejected"])
            )),
            Cell::from(fmt_duration(s["median_ms"].as_f64().map(|ms| ms / 1000.0))),
            Cell::from(fmt_usd(&s["mean_usd"])),
            Cell::from(
                s["score"]
                    .as_f64()
                    .map_or_else(|| DASH.to_owned(), |x| format!("{x:.2}")),
            ),
        ])
        .style(Style::default().fg(TEXT))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(6),
            Constraint::Min(6),
            Constraint::Min(8),
            Constraint::Length(4),
            Constraint::Length(9),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(5),
        ],
    )
    .header(header_row(&[
        "種類",
        "実行者",
        "モデル",
        "n",
        "成功",
        "採/否",
        "中央値",
        "平均費用",
        "score",
    ]))
    .column_spacing(1);
    frame.render_widget(table, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn unknown_cost_is_never_zero() {
        assert_eq!(fmt_usd(&Value::Null), "不明");
        assert_eq!(fmt_usd(&json!({})["reported_usd"]), "不明");
        assert_eq!(fmt_usd(&json!(0.0)), "$0.00");
        assert_eq!(fmt_usd(&json!(0)), "$0.00");
        assert_eq!(fmt_usd(&json!(0.1234)), "$0.12");
        assert_eq!(fmt_usd(&json!(0.0042)), "$0.0042");
    }

    #[test]
    fn missing_values_render_as_dash() {
        assert_eq!(
            fmt_usage(&json!({"input_tokens": 12_345, "output_tokens": 800})),
            "入力 12.3k · 出力 800 · cache読 — · cache書 —"
        );
        assert_eq!(
            fmt_usage(&Value::Null),
            "入力 — · 出力 — · cache読 — · cache書 —"
        );
        assert_eq!(fmt_duration(None), "—");
        assert_eq!(fmt_duration(Some(42.4)), "42s");
        assert_eq!(fmt_duration(Some(185.0)), "3m05s");
        assert_eq!(fmt_duration(Some(3720.0)), "1h02m");
        assert_eq!(text_or_dash(&Value::Null), "—");
        assert_eq!(text_or_dash(&json!("")), "—");
        assert_eq!(num_or_dash(&json!("x")), "—");
        assert_eq!(list_or_dash(&json!([])), "—");
        assert_eq!(
            timestamp(&json!("2026-09-24T10:00:00Z")),
            "2026-09-24 10:00:00Z"
        );
    }

    #[test]
    fn ledger_quota_formatting() {
        assert_eq!(fmt_count_quota(&json!(3), &json!(20)), "3/20 件");
        assert_eq!(fmt_count_quota(&json!(3), &Value::Null), "3 件");
        assert_eq!(fmt_count_quota(&Value::Null, &Value::Null), "0 件");
        assert_eq!(
            fmt_usd_quota(&json!(0.12), &json!(0), &json!(1.0)),
            "$0.12/$1.00"
        );
        assert_eq!(
            fmt_usd_quota(&json!(0.0), &json!(2), &Value::Null),
            "$0.00 (+2 件不明)"
        );
        assert_eq!(
            fmt_usd_quota(&Value::Null, &Value::Null, &Value::Null),
            "$0.00"
        );
        assert_eq!(
            fmt_tokens_quota(&json!(1500), &json!(100_000)),
            "1.5k/100.0k"
        );
        assert_eq!(fmt_tokens_quota(&json!(2_500_000), &Value::Null), "2.5M");
        let total = sum_usage(&json!({
            "a": {"tasks": 2, "usd": 0.5, "tokens": 10, "unpriced": 1},
            "b": {"tasks": 1, "usd": 0.25}
        }));
        assert_eq!(total["tasks"], 3);
        assert_eq!(total["unpriced"], 1);
        assert_eq!(fmt_usd(&total["usd"]), "$0.75");
    }

    #[test]
    fn filtering_sorts_newest_first_and_keeps_active_only_on_demand() {
        let panel = sample_panel();
        let all: Vec<&str> = filtered_tasks(Some(&panel), false)
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(all, ["t3", "t2", "t1"]);
        let running: Vec<&str> = filtered_tasks(Some(&panel), true)
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(running, ["t3"]);
        assert!(filtered_tasks(None, false).is_empty());
        assert!(filtered_tasks(Some(&json!({})), true).is_empty());
        assert_eq!(running_count(Some(&panel)), 1);
    }

    #[test]
    fn selection_follows_id_then_clamps() {
        let ids = ["a", "b", "c"];
        assert_eq!(resolve_selection(&ids, Some("c"), 0), Some(2));
        // A new task on top keeps the same task selected.
        assert_eq!(resolve_selection(&["n", "a", "b"], Some("a"), 0), Some(1));
        // Vanished id: clamp the old index.
        assert_eq!(resolve_selection(&ids, Some("gone"), 7), Some(2));
        assert_eq!(resolve_selection(&ids, None, 1), Some(1));
        assert_eq!(resolve_selection(&[], Some("a"), 3), None);
    }

    #[test]
    fn action_availability() {
        let panel = sample_panel();
        let tasks = filtered_tasks(Some(&panel), false);
        let (running, done, failed) = (tasks[0], tasks[1], tasks[2]);
        assert!(is_active(running) && !is_finished(running));
        assert!(can_continue(done) && has_worktree(done));
        assert!(is_finished(failed) && !can_continue(failed) && !has_worktree(failed));
        assert_eq!(reply_text(&json!({"message": "ok"})), "ok");
    }

    fn sample_panel() -> Value {
        json!({
            "health": {
                "executors": [
                    {"name": "claude-code", "adapter": "cli", "protocol": "stream-json",
                     "enabled": true, "ok": true, "running": 1, "max_concurrent": 2,
                     "default_model": "opus", "models": ["opus", "sonnet"],
                     "supervisor": {"kind": "app-server", "state": "running", "connections": 1}},
                    {"name": "codex", "protocol": "app-server", "enabled": false, "ok": false}
                ],
                "running": 1, "queued_or_running": 1, "max_concurrent": 4,
                "routing_mode": "manual"
            },
            "tasks": [
                {"id": "t1", "title": "壊れたテストの調査", "status": "failed",
                 "created": "2026-09-24T09:00:00Z",
                 "detail": {"executor": "codex", "model": "gpt"},
                 "result": {"summary": "", "error": "timeout", "cost": {}}},
                {"id": "t2", "title": "README の誤字修正", "status": "done",
                 "created": "2026-09-24T10:00:00Z", "duration_secs": 185,
                 "detail": {"executor": "claude-code", "model": "opus",
                            "worktree": "/tmp/wt", "branch": "agent/t2"},
                 "notes": [{"at": "2026-09-24T10:01:00Z", "by": "orchestrator", "text": "started"}],
                 "result": {"summary": "fixed typo\nsecond line", "session_id": "s-1",
                            "usage": {"input_tokens": 1200, "output_tokens": 30},
                            "cost": {"reported_usd": null, "estimated_usd": 0.0, "billing": "subscription"},
                            "files_changed": ["README.md"], "tool_activity": ["Edit README.md"],
                            "read_only_violation": true, "feedback": "accept"}},
                {"id": "t3", "title": "依存関係の更新", "status": "in_progress",
                 "created": "2026-09-24T11:00:00Z",
                 "detail": {"executor": "claude-code", "model": "sonnet"}, "result": null}
            ],
            "ledger": {
                "day": "2026-09-24", "month": "2026-09",
                "today": {"claude-code": {"tasks": 2, "usd": 0.5, "tokens": 1500, "unpriced": 1}},
                "this_month": {"claude-code": {"tasks": 9, "usd": 3.25, "tokens": 90000}},
                "quotas": {"claude-code": {"max_tasks_per_day": 20, "max_usd_per_day": 5.0}}
            },
            "stats": [
                {"task_kind": "fix", "executor": "claude-code", "model": "opus", "n": 4,
                 "succeeded": 3, "accepted": 2, "rejected": 1, "median_ms": 90000,
                 "mean_usd": null, "score": 0.75}
            ]
        })
    }

    /// Buffer rows as text; the trailing cell of a wide glyph is skipped.
    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut skip = 0;
            for x in 0..buffer.area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let symbol = buffer[(x, y)].symbol();
                skip = symbol.width().saturating_sub(1);
                out.push_str(symbol);
            }
            out.push('\n');
        }
        out
    }

    fn test_app() -> App {
        let (tx, _rx) = std::sync::mpsc::channel();
        App::new(tx, "tester".to_owned(), Some("agents".to_owned()))
    }

    #[test]
    fn renders_panel_headlessly() {
        let mut app = test_app();
        app.set_agents(sample_panel());
        // Select the finished task (second, newest first).
        app.agent_sel = 1;
        let mut terminal = Terminal::new(TestBackend::new(160, 64)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text = screen(&terminal);
        for needle in [
            "claude-code",
            "codex",
            "無効",
            "監督 app-server running",
            "依存関係の更新",
            "README の誤字修正",
            "報告 不明 · 推定 $0.00",
            "読み取り専用のはずのタスク",
            "fixed typo",
            "A: worktree 適用",
            "利用台帳",
            "2/20 件",
            "(+1 件不明)",
            "実績",
            "0.75",
            "6 エージェント",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
        assert_eq!(app.agent_sel_id.as_deref(), Some("t2"));
    }

    #[test]
    fn keys_send_task_plane_actions() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use kamimusuhi_resident::remote::RemoteCommand;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App::new(tx, "tester".to_owned(), Some("agents".to_owned()));
        app.set_agents(sample_panel());
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let sent = |rx: &std::sync::mpsc::Receiver<RemoteCommand>| match rx.try_recv() {
            Ok(RemoteCommand::Agents(body)) => Some(body),
            _ => None,
        };
        // Newest first: t3 (in_progress) is selected; cancel is allowed.
        app.on_key(key(KeyCode::Char('c')));
        assert_eq!(sent(&rx), Some(json!({"action": "cancel", "id": "t3"})));
        // Feedback on a running task is refused locally.
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(sent(&rx), None);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.agent_sel_id.as_deref(), Some("t2"));
        app.on_key(key(KeyCode::Char('r')));
        assert_eq!(
            sent(&rx),
            Some(json!({"action": "feedback", "id": "t2", "verdict": "reject"}))
        );
        app.on_key(key(KeyCode::Char('A')));
        assert_eq!(sent(&rx), Some(json!({"action": "apply", "id": "t2"})));
        app.on_key(key(KeyCode::Char('n')));
        for c in "もう一度".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            sent(&rx),
            Some(json!({"action": "continue", "id": "t2", "instruction": "もう一度"}))
        );
        // Selection clamps at the end and survives the running-only toggle.
        for _ in 0..5 {
            app.on_key(key(KeyCode::Down));
        }
        assert_eq!(app.agent_sel_id.as_deref(), Some("t1"));
        app.on_key(key(KeyCode::Char('h')));
        assert_eq!(app.selected_agent_task().unwrap()["id"], "t3");
    }

    #[test]
    fn renders_absent_task_plane() {
        let mut app = test_app();
        app.set_agents(Value::Null);
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(screen(&terminal).contains("この node には Task Plane がありません"));
    }
}
