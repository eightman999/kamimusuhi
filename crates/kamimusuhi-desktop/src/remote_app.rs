//! Resident-mode window: conversation with the always-on individual, its
//! approval queue, system status and tool catalog.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Stroke};
use eframe::{App, Frame, NativeOptions};
use serde_json::Value;

use crate::app::{
    AMBER, BG, BLUE, BORDER, GREEN, MUTED, PANEL, PANEL_RAISED, TEXT, card, configure_theme,
};
use crate::remote::{RemoteCommand, RemoteEvent};

const RED: Color32 = Color32::from_rgb(235, 96, 110);
const VIOLET: Color32 = Color32::from_rgb(166, 138, 255);
/// The individual accepts at most this much per turn (runtime contract).
const MAX_INPUT_BYTES: usize = kamimusuhi_runtime::dialogue::MAX_INPUT_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Tasks,
    Approvals,
    Status,
    Tools,
}

#[derive(Debug, Clone)]
enum Entry {
    User { text: String, at: Option<String> },
    Assistant { text: String, meta: Option<Value> },
    Notice { text: String, color: Color32 },
}

pub fn run(
    commands: Sender<RemoteCommand>,
    events: Receiver<RemoteEvent>,
    subject: String,
    initial_view: Option<&str>,
) -> Result<(), String> {
    let view = match initial_view {
        Some("tasks" | "map") => View::Tasks,
        Some("approvals") => View::Approvals,
        Some("status") => View::Status,
        Some("tools") => View::Tools,
        _ => View::Chat,
    };
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 880.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("澪 (Mio) — Kamimusuhi 常駐個体"),
        ..Default::default()
    };
    eframe::run_native(
        "Kamimusuhi Resident",
        options,
        Box::new(move |cc| {
            configure_theme(&cc.egui_ctx);
            let mut app = RemoteApp::new(commands, events, subject);
            app.view = view;
            app.map_mode = initial_view == Some("map");
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

struct RemoteApp {
    commands: Sender<RemoteCommand>,
    events: Receiver<RemoteEvent>,
    subject: String,
    url: Option<String>,
    view: View,
    entries: Vec<Entry>,
    input: String,
    busy_since: Option<Instant>,
    error: Option<String>,
    status: Option<Value>,
    status_at: Option<Instant>,
    approvals: Vec<Value>,
    announced: HashSet<String>,
    deciding: HashSet<String>,
    last_decisions: Vec<(String, Value)>,
    tools: Option<Value>,
    tool_filter: String,
    tasks: Vec<Value>,
    selected_task: Option<String>,
    show_done: bool,
    new_task_title: String,
    new_task_after_selected: bool,
    task_note: String,
    map_mode: bool,
}

impl RemoteApp {
    fn new(
        commands: Sender<RemoteCommand>,
        events: Receiver<RemoteEvent>,
        subject: String,
    ) -> Self {
        Self {
            commands,
            events,
            subject,
            url: None,
            view: View::Chat,
            entries: Vec::new(),
            input: String::new(),
            busy_since: None,
            error: None,
            status: None,
            status_at: None,
            approvals: Vec::new(),
            announced: HashSet::new(),
            deciding: HashSet::new(),
            last_decisions: Vec::new(),
            tools: None,
            tool_filter: String::new(),
            tasks: Vec::new(),
            selected_task: None,
            show_done: true,
            new_task_title: String::new(),
            new_task_after_selected: false,
            task_note: String::new(),
            map_mode: false,
        }
    }

    fn pending(&self) -> Vec<&Value> {
        self.approvals
            .iter()
            .filter(|a| a["state"] == "pending")
            .collect()
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                RemoteEvent::Connected { url, history } => {
                    self.url = Some(url);
                    self.error = None;
                    for turn in history {
                        self.entries.push(Entry::User {
                            text: turn["message"].as_str().unwrap_or("").to_owned(),
                            at: turn["ts"].as_str().map(str::to_owned),
                        });
                        self.entries.push(Entry::Assistant {
                            text: turn["response"].as_str().unwrap_or("").to_owned(),
                            meta: Some(turn),
                        });
                    }
                    if !self.entries.is_empty() {
                        self.entries.push(Entry::Notice {
                            text: "── ここまでが保存済みの会話です ──".to_owned(),
                            color: MUTED,
                        });
                    }
                }
                RemoteEvent::Reply(reply) => {
                    self.busy_since = None;
                    self.error = None;
                    self.entries.push(Entry::Assistant {
                        text: reply["response"].as_str().unwrap_or("").to_owned(),
                        meta: Some(reply),
                    });
                }
                RemoteEvent::Status(status) => {
                    self.status = Some(status);
                    self.status_at = Some(Instant::now());
                }
                RemoteEvent::Approvals(list) => {
                    let first_load = self.announced.is_empty()
                        && self.url.is_some()
                        && self.approvals.is_empty();
                    for a in list.iter().filter(|a| a["state"] == "pending") {
                        let id = a["id"].as_str().unwrap_or("").to_owned();
                        if self.announced.insert(id) && !first_load {
                            self.entries.push(Entry::Notice {
                                text: format!("⚠ 承認待ちが追加されました: {}", approval_title(a)),
                                color: AMBER,
                            });
                        }
                    }
                    self.approvals = list;
                }
                RemoteEvent::Tools(tools) => self.tools = Some(tools),
                RemoteEvent::Tasks(tasks) => self.tasks = tasks,
                RemoteEvent::Decision { id, reply } => {
                    self.deciding.remove(&id);
                    let ok = reply["ok"].as_bool().unwrap_or(false);
                    self.entries.push(Entry::Notice {
                        text: format!(
                            "{} 承認 {id}: {}",
                            if ok { "✔" } else { "✘" },
                            reply["state"]
                                .as_str()
                                .or_else(|| reply["approval"]["state"].as_str())
                                .unwrap_or("?")
                        ),
                        color: if ok { GREEN } else { RED },
                    });
                    self.last_decisions.insert(0, (id, reply));
                    self.last_decisions.truncate(10);
                }
                RemoteEvent::Error(message) => {
                    if message.starts_with("応答") {
                        self.busy_since = None;
                    }
                    self.error = Some(message);
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(if self.busy_since.is_some() {
            100
        } else {
            500
        }));
    }

    fn send(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() || self.busy_since.is_some() || self.url.is_none() {
            return;
        }
        if text.len() > MAX_INPUT_BYTES {
            self.error = Some(format!("入力が {MAX_INPUT_BYTES} bytes を超えています。"));
            return;
        }
        self.entries.push(Entry::User {
            text: text.clone(),
            at: None,
        });
        self.input.clear();
        self.error = None;
        self.busy_since = Some(Instant::now());
        if self.commands.send(RemoteCommand::Send { text }).is_err() {
            self.busy_since = None;
            self.error = Some("通信ワーカーが停止しています。".to_owned());
        }
    }

    fn decide(&mut self, id: &str, approve: bool) {
        if self.deciding.insert(id.to_owned()) {
            let _ = self.commands.send(RemoteCommand::Decide {
                id: id.to_owned(),
                approve,
            });
        }
    }

    // ── chrome ──────────────────────────────────────────────────────────

    fn top_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("澪").strong().size(19.0).color(TEXT));
            ui.label(RichText::new("常駐個体").small().color(MUTED));
            ui.add_space(14.0);
            let node_line = self.status.as_ref().map_or_else(
                || "接続中…".to_owned(),
                |s| {
                    format!(
                        "{} · 経路 {} · subject {}",
                        s["node"]["node"].as_str().unwrap_or("?"),
                        s["routing"]["active_primary"].as_str().unwrap_or("なし"),
                        self.subject
                    )
                },
            );
            ui.label(RichText::new(node_line).small().color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, color) = match (&self.url, self.busy_since) {
                    (None, _) => ("○ 接続待ち".to_owned(), MUTED),
                    (Some(_), Some(t)) => (format!("考え中 {}s", t.elapsed().as_secs()), AMBER),
                    (Some(_), None) => ("● オンライン".to_owned(), GREEN),
                };
                ui.label(RichText::new(label).small().color(color));
                let pending = self.pending().len();
                if pending > 0 {
                    ui.label(
                        RichText::new(format!("承認待ち {pending}"))
                            .small()
                            .strong()
                            .color(AMBER),
                    );
                }
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("KAMIMUSUHI").small().strong().color(MUTED));
        ui.label(RichText::new("Pi / llm_master 常駐").small().color(MUTED));
        ui.add_space(18.0);
        let pending = self.pending().len();
        for (label, view) in [
            ("▣  対話".to_owned(), View::Chat),
            (
                {
                    let running = self
                        .tasks
                        .iter()
                        .filter(|t| t["status"] == "in_progress")
                        .count();
                    if running > 0 {
                        format!("☰  タスク  ({running})")
                    } else {
                        "☰  タスク".to_owned()
                    }
                },
                View::Tasks,
            ),
            (
                if pending > 0 {
                    format!("✋  承認  ({pending})")
                } else {
                    "✋  承認".to_owned()
                },
                View::Approvals,
            ),
            ("◌  状態".to_owned(), View::Status),
            ("⚒  ツール".to_owned(), View::Tools),
        ] {
            let selected = self.view == view;
            let color = if view == View::Approvals && pending > 0 {
                AMBER
            } else if selected {
                TEXT
            } else {
                MUTED
            };
            if ui
                .add_sized(
                    [ui.available_width(), 34.0],
                    egui::Button::new(RichText::new(label).color(color))
                        .fill(if selected {
                            Color32::from_rgb(24, 62, 111)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(if selected {
                            Stroke::new(1.0, Color32::from_rgb(46, 108, 185))
                        } else {
                            Stroke::NONE
                        }),
                )
                .clicked()
            {
                self.view = view;
            }
        }
        ui.add_space(14.0);
        ui.separator();
        if let Some(s) = &self.status {
            ui.add_space(8.0);
            health_line(ui, "Pi", true);
            for (id, peer) in s["peers"].as_object().into_iter().flatten() {
                health_line(ui, id, peer["healthy"].as_bool().unwrap_or(false));
            }
            health_line(
                ui,
                "NAS",
                s["nas"]["probe"]["healthy"].as_bool().unwrap_or(false),
            );
            health_line(
                ui,
                "HAI",
                s["routing"]["tiers"]["hai"]["healthy"]
                    .as_bool()
                    .unwrap_or(false),
            );
            let mcp_ok = s["mcp"]
                .as_array()
                .is_some_and(|m| m.iter().all(|x| x["state"] == "running"));
            health_line(ui, "MCP", mcp_ok);
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if ui.button("↻ 更新").clicked() {
                let _ = self.commands.send(RemoteCommand::Refresh);
            }
            ui.label(
                RichText::new(self.url.clone().unwrap_or_default())
                    .small()
                    .color(MUTED),
            );
        });
    }

    // ── views ───────────────────────────────────────────────────────────

    fn chat(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let composer = 112.0 + if self.error.is_some() { 44.0 } else { 0.0 };
        let height = (ui.available_height() - composer).max(120.0);
        let width = ui.available_width();
        let mut decisions = Vec::new();
        ui.allocate_ui(egui::vec2(width, height), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("remote-chat")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.entries.is_empty() {
                        ui.add_space(80.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("澪と話す").size(22.0).color(TEXT));
                            ui.label(
                                RichText::new("Pi の常駐個体が記憶と連続性を持って応答します。必要に応じて参照データや MCP ツールを使います。")
                                    .color(MUTED),
                            );
                        });
                    }
                    for entry in &self.entries {
                        match entry {
                            Entry::User { text, at } => bubble(ui, true, text, None, at.as_deref()),
                            Entry::Assistant { text, meta } => bubble(ui, false, text, meta.as_ref(), None),
                            Entry::Notice { text, color } => {
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new(text).small().color(*color));
                                });
                            }
                        }
                        ui.add_space(8.0);
                    }
                    // Pending approvals are actionable right in the conversation.
                    for a in self.pending() {
                        let id = a["id"].as_str().unwrap_or("").to_owned();
                        if let Some(approve) = approval_card(ui, a, self.deciding.contains(&id), true) {
                            decisions.push((id, approve));
                        }
                        ui.add_space(8.0);
                    }
                    if let Some(t) = self.busy_since {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new(format!(
                                    "考えています… {}s（参照・ツール使用時は 1〜2 分かかることがあります）",
                                    t.elapsed().as_secs()
                                ))
                                .small()
                                .color(MUTED),
                            );
                        });
                    }
                });
        });
        for (id, approve) in decisions {
            self.decide(&id, approve);
        }
        if let Some(error) = &self.error {
            egui::Frame::new()
                .fill(Color32::from_rgb(66, 28, 37))
                .stroke(Stroke::new(1.0, Color32::from_rgb(145, 53, 69)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(error).color(Color32::from_rgb(255, 191, 196)));
                });
            ui.add_space(6.0);
        }
        egui::Frame::new()
            .fill(PANEL_RAISED)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(8.0)
            .show(ui, |ui| {
                let enabled = self.busy_since.is_none() && self.url.is_some();
                let edit = egui::TextEdit::multiline(&mut self.input)
                    .hint_text("メッセージを入力…  (⌘/Ctrl + Enter で送信)")
                    .desired_rows(3)
                    .desired_width((ui.available_width() - 92.0).max(240.0));
                let response = ui.add_enabled(enabled, edit);
                let shortcut = response.has_focus()
                    && ctx.input(|i| {
                        i.key_pressed(egui::Key::Enter) && (i.modifiers.ctrl || i.modifiers.command)
                    });
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Enter: 改行 / ⌘+Enter: 送信")
                            .small()
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(
                                    RichText::new("送信  ➤").strong().color(Color32::WHITE),
                                )
                                .fill(BLUE),
                            )
                            .clicked()
                            || shortcut
                        {
                            self.send();
                        }
                    });
                });
            });
    }

    fn approvals_view(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("承認").color(TEXT));
        ui.label(
            RichText::new("個体が要求した書き換え操作です。承認するとその内容のまま実行され、Obsidian の場合は commit/push されます。")
                .small()
                .color(MUTED),
        );
        ui.separator();
        let mut decisions = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("approvals")
            .show(ui, |ui| {
                let pending = self.pending();
                if pending.is_empty() {
                    ui.label(RichText::new("承認待ちはありません。").color(MUTED));
                }
                for a in pending {
                    let id = a["id"].as_str().unwrap_or("").to_owned();
                    if let Some(approve) = approval_card(ui, a, self.deciding.contains(&id), false)
                    {
                        decisions.push((id, approve));
                    }
                    ui.add_space(10.0);
                }
                ui.add_space(12.0);
                ui.label(RichText::new("最近の決定").strong().color(TEXT));
                for a in self
                    .approvals
                    .iter()
                    .filter(|a| a["state"] != "pending")
                    .take(20)
                {
                    let color = match a["state"].as_str() {
                        Some("done") => GREEN,
                        Some("failed") => RED,
                        _ => MUTED,
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(a["state"].as_str().unwrap_or("?"))
                                .small()
                                .strong()
                                .color(color),
                        );
                        ui.label(RichText::new(approval_title(a)).small().color(TEXT));
                        ui.label(
                            RichText::new(a["decided"].as_str().unwrap_or(""))
                                .small()
                                .color(MUTED),
                        );
                    });
                    if let Some(err) = a["result"].get("error") {
                        ui.label(RichText::new(format!("   error: {err}")).small().color(RED));
                    }
                    if let Some(hook) = a["result"].get("after_approved") {
                        let line = hook["stdout"]
                            .as_array()
                            .and_then(|l| l.first())
                            .and_then(Value::as_str)
                            .or_else(|| {
                                hook["stderr"]
                                    .as_array()
                                    .and_then(|l| l.first())
                                    .and_then(Value::as_str)
                            })
                            .unwrap_or("");
                        ui.label(
                            RichText::new(format!("   after_approved: {line}"))
                                .small()
                                .color(MUTED),
                        );
                    }
                }
            });
        for (id, approve) in decisions {
            self.decide(&id, approve);
        }
    }

    fn status_view(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("状態").color(TEXT));
            if let Some(t) = self.status_at {
                ui.label(
                    RichText::new(format!("{}秒前に更新", t.elapsed().as_secs()))
                        .small()
                        .color(MUTED),
                );
            }
        });
        ui.separator();
        let Some(s) = &self.status else {
            ui.spinner();
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("status")
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    card(&mut cols[0], "ノード", |ui| {
                        let n = &s["node"];
                        node_row(
                            ui,
                            n["node"].as_str().unwrap_or("?"),
                            true,
                            &format!(
                                "{} · 起動 {}s · epoch {} · K-CORE {}",
                                n["role"].as_str().unwrap_or(""),
                                n["uptime_secs"],
                                n["boot_epoch"],
                                n["kcore"].as_str().unwrap_or("-")
                            ),
                        );
                        for (id, p) in s["peers"].as_object().into_iter().flatten() {
                            let ok = p["healthy"].as_bool().unwrap_or(false);
                            let d = &p["detail"];
                            let gpu = d["gpu"]
                                .as_array()
                                .map(|g| {
                                    g.iter()
                                        .map(|x| {
                                            format!(
                                                "{} {}/{}MiB",
                                                x["name"].as_str().unwrap_or(""),
                                                x["memory_used_mib"],
                                                x["memory_total_mib"]
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            node_row(
                                ui,
                                id,
                                ok,
                                &if ok {
                                    format!(
                                        "{} · local LLM {} · {}",
                                        d["role"].as_str().unwrap_or(""),
                                        d["local_llm"],
                                        gpu
                                    )
                                } else {
                                    format!("DOWN: {}", p["error"].as_str().unwrap_or(""))
                                },
                            );
                        }
                    });
                    cols[0].add_space(8.0);
                    card(&mut cols[0], "ルーティング", |ui| {
                        let r = &s["routing"];
                        ui.label(
                            RichText::new(format!(
                                "Primary: {}",
                                r["active_primary"].as_str().unwrap_or("なし")
                            ))
                            .color(TEXT),
                        );
                        for (name, t) in r["tiers"].as_object().into_iter().flatten() {
                            let ok = t["healthy"].as_bool().unwrap_or(false);
                            node_row(
                                ui,
                                name,
                                ok,
                                &if ok {
                                    format!("{}ms", t["latency_ms"])
                                } else {
                                    t["error"].as_str().unwrap_or("").to_owned()
                                },
                            );
                        }
                        if let Some(last) = r.get("last_route").filter(|v| !v.is_null()) {
                            ui.label(
                                RichText::new(format!(
                                    "直近: {} {}ms",
                                    last["tier"].as_str().unwrap_or("-"),
                                    last["latency_ms"]
                                ))
                                .small()
                                .color(MUTED),
                            );
                        }
                    });
                    cols[0].add_space(8.0);
                    card(&mut cols[0], "NAS", |ui| {
                        let n = &s["nas"];
                        node_row(
                            ui,
                            n["root"].as_str().unwrap_or("-"),
                            n["probe"]["healthy"].as_bool().unwrap_or(false),
                            &format!(
                                "未配送 {} files / {} bytes · 最終同期 {}",
                                n["spool_backlog_files"],
                                n["spool_backlog_bytes"],
                                n["sync"]["last_success"].as_str().unwrap_or("-")
                            ),
                        );
                    });
                    card(&mut cols[1], "HAI", |ui| {
                        let t = &s["routing"]["tiers"]["hai"];
                        node_row(
                            ui,
                            "API",
                            t["healthy"].as_bool().unwrap_or(false),
                            &format!("{}ms", t["latency_ms"]),
                        );
                        ui.label(
                            RichText::new(format!(
                                "残高表示 {} JPY（サブスク）",
                                s["hai_account"]["detail"]["balance_jpy"]
                                    .as_str()
                                    .unwrap_or("-")
                            ))
                            .small()
                            .color(MUTED),
                        );
                    });
                    cols[1].add_space(8.0);
                    card(&mut cols[1], "MCP サーバー", |ui| {
                        for m in s["mcp"].as_array().into_iter().flatten() {
                            let ok = m["state"] == "running";
                            node_row(
                                ui,
                                m["name"].as_str().unwrap_or("?"),
                                ok,
                                &if ok {
                                    format!("{} tools", m["tools"])
                                } else {
                                    m["error"].as_str().unwrap_or("").to_owned()
                                },
                            );
                        }
                    });
                    cols[1].add_space(8.0);
                    card(&mut cols[1], "定期ジョブ / 記憶", |ui| {
                        for (name, j) in s["jobs"].as_object().into_iter().flatten() {
                            node_row(
                                ui,
                                name,
                                j["ok"].as_bool().unwrap_or(false),
                                &format!("最終 {}", j["last_run"].as_str().unwrap_or("-")),
                            );
                        }
                        let snap = &s["snapshot"];
                        node_row(
                            ui,
                            "snapshot",
                            snap["ok"].as_bool().unwrap_or(false),
                            snap["at"].as_str().unwrap_or("-"),
                        );
                    });
                });
            });
    }

    fn tools_view(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("ツール").color(TEXT));
        ui.label(
            RichText::new("個体が使える参照ライブラリと MCP サーバー（Pi と llm_master の合計）")
                .small()
                .color(MUTED),
        );
        ui.separator();
        let Some(tools) = self.tools.clone() else {
            ui.spinner();
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("tools")
            .show(ui, |ui| {
                card(ui, "ライブラリ", |ui| {
                    for l in tools["libraries"].as_array().into_iter().flatten() {
                        ui.label(
                            RichText::new(format!(
                                "{}  @{}",
                                l["name"].as_str().unwrap_or("?"),
                                l["node"].as_str().unwrap_or("?")
                            ))
                            .strong()
                            .color(TEXT),
                        );
                        ui.label(
                            RichText::new(l["description"].as_str().unwrap_or(""))
                                .small()
                                .color(MUTED),
                        );
                    }
                });
                ui.add_space(8.0);
                card(ui, "MCP サーバー", |ui| {
                    for m in tools["mcp_servers"].as_array().into_iter().flatten() {
                        let ok = m["state"] == "running";
                        node_row(
                            ui,
                            &format!(
                                "{} @{}",
                                m["name"].as_str().unwrap_or("?"),
                                m["node"].as_str().unwrap_or("?")
                            ),
                            ok,
                            &format!(
                                "{} tools · {}",
                                m["tools"],
                                m["description"].as_str().unwrap_or("")
                            ),
                        );
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("tool 一覧").strong().color(TEXT));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tool_filter)
                            .hint_text("絞り込み")
                            .desired_width(220.0),
                    );
                });
                let filter = self.tool_filter.to_lowercase();
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
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(name).monospace().color(if gated {
                            AMBER
                        } else {
                            VIOLET
                        }));
                        ui.label(
                            RichText::new(desc.chars().take(140).collect::<String>())
                                .small()
                                .color(MUTED),
                        );
                    });
                }
            });
    }
}

impl App for RemoteApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.poll(ctx);
        egui::TopBottomPanel::top("remote-top")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(12.0))
            .show(ctx, |ui| self.top_bar(ui));
        egui::SidePanel::left("remote-nav")
            .frame(egui::Frame::new().fill(BG).inner_margin(12.0))
            .resizable(false)
            .default_width(190.0)
            .show(ctx, |ui| self.nav(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(16.0))
            .show(ctx, |ui| match self.view {
                View::Chat => self.chat(ui, ctx),
                View::Tasks => self.tasks_view(ui),
                View::Approvals => self.approvals_view(ui),
                View::Status => self.status_view(ui),
                View::Tools => self.tools_view(ui),
            });
    }
}

impl Drop for RemoteApp {
    fn drop(&mut self) {
        let _ = self.commands.send(RemoteCommand::Shutdown);
    }
}

// ── widgets ────────────────────────────────────────────────────────────────

fn health_line(ui: &mut egui::Ui, label: &str, ok: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("●").color(if ok { GREEN } else { RED }));
        ui.label(
            RichText::new(label)
                .small()
                .color(if ok { TEXT } else { RED }),
        );
    });
}

fn node_row(ui: &mut egui::Ui, name: &str, ok: bool, detail: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("●").color(if ok { GREEN } else { RED }));
        ui.label(RichText::new(name).strong().color(TEXT));
        ui.label(RichText::new(detail).small().color(MUTED));
    });
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        });
}

fn bubble(ui: &mut egui::Ui, user: bool, text: &str, meta: Option<&Value>, at: Option<&str>) {
    let (fill, stroke) = if user {
        (
            Color32::from_rgb(25, 75, 145),
            Color32::from_rgb(45, 121, 222),
        )
    } else {
        (PANEL_RAISED, BORDER)
    };
    ui.with_layout(
        if user {
            egui::Layout::right_to_left(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Min)
        },
        |ui| {
            egui::Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(760.0));
                    // The parent row is left/right aligned; the bubble body
                    // itself must stack top-to-bottom and wrap.
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if user { "あなた" } else { "澪" })
                                    .small()
                                    .strong()
                                    .color(if user {
                                        Color32::from_rgb(183, 218, 255)
                                    } else {
                                        Color32::from_rgb(133, 219, 184)
                                    }),
                            );
                            if let Some(ts) = at.or_else(|| meta.and_then(|m| m["ts"].as_str())) {
                                ui.label(
                                    RichText::new(
                                        ts.replace('T', " ").trim_end_matches('Z').to_owned(),
                                    )
                                    .small()
                                    .color(MUTED),
                                );
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(text).color(TEXT));
                        if let Some(meta) = meta {
                            let tools = meta["tool_calls"].as_array().cloned().unwrap_or_default();
                            let tier = meta["tier"].as_str();
                            let latency = meta["latency_ms"].as_u64();
                            let lookups = meta["reference_lookups"].as_u64().unwrap_or(0);
                            if tier.is_some()
                                || latency.is_some()
                                || !tools.is_empty()
                                || lookups > 0
                            {
                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    if let Some(tier) = tier {
                                        chip(
                                            ui,
                                            &format!("via {tier}"),
                                            if tier == "hai" { AMBER } else { GREEN },
                                        );
                                    }
                                    if let Some(ms) = latency {
                                        chip(ui, &format!("{:.1}s", ms as f64 / 1000.0), MUTED);
                                    }
                                    if lookups > 0 {
                                        chip(ui, &format!("参照 {lookups}"), BLUE);
                                    }
                                    let mut counts: Vec<(String, usize, bool)> = Vec::new();
                                    for t in &tools {
                                        let name = t["name"]
                                            .as_str()
                                            .unwrap_or("?")
                                            .trim_start_matches("mcp__")
                                            .replace("__", "/");
                                        let ok = t["ok"].as_bool().unwrap_or(true);
                                        if let Some(entry) =
                                            counts.iter_mut().find(|(n, _, _)| *n == name)
                                        {
                                            entry.1 += 1;
                                            entry.2 &= ok;
                                        } else {
                                            counts.push((name, 1, ok));
                                        }
                                    }
                                    for (name, n, ok) in counts {
                                        let label = if n > 1 {
                                            format!("🔧 {name} ×{n}")
                                        } else {
                                            format!("🔧 {name}")
                                        };
                                        chip(ui, &label, if ok { VIOLET } else { RED });
                                    }
                                });
                            }
                        }
                    });
                });
        },
    );
}

fn approval_title(a: &Value) -> String {
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

/// Returns `Some(true)` for approve, `Some(false)` for reject.
fn approval_card(ui: &mut egui::Ui, a: &Value, deciding: bool, compact: bool) -> Option<bool> {
    let mut decision = None;
    egui::Frame::new()
        .fill(Color32::from_rgb(40, 34, 18))
        .stroke(Stroke::new(1.0, AMBER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(820.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("✋ 承認待ち").strong().color(AMBER));
                ui.label(RichText::new(approval_title(a)).strong().color(TEXT));
                ui.label(
                    RichText::new(a["created"].as_str().unwrap_or(""))
                        .small()
                        .color(MUTED),
                );
            });
            let args = &a["arguments"];
            let content = ["content", "newString", "text"]
                .iter()
                .find_map(|k| args.get(*k).and_then(Value::as_str));
            if let Some(content) = content {
                egui::ScrollArea::vertical()
                    .id_salt(("approval-content", a["id"].as_str().unwrap_or("")))
                    .max_height(if compact { 120.0 } else { 260.0 })
                    .show(ui, |ui| {
                        ui.label(RichText::new(content).monospace().color(TEXT));
                    });
            }
            egui::CollapsingHeader::new(RichText::new("引数（JSON）").small().color(MUTED))
                .id_salt(("approval-args", a["id"].as_str().unwrap_or("")))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(serde_json::to_string_pretty(args).unwrap_or_default())
                            .monospace()
                            .small()
                            .color(MUTED),
                    );
                });
            ui.horizontal(|ui| {
                if deciding {
                    ui.spinner();
                    ui.label(RichText::new("実行中…").small().color(MUTED));
                } else {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("承認して実行").strong().color(Color32::BLACK),
                            )
                            .fill(GREEN),
                        )
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("却下").color(TEXT)).fill(PANEL_RAISED),
                        )
                        .clicked()
                    {
                        decision = Some(false);
                    }
                }
                ui.label(
                    RichText::new(format!("ID {}", a["id"].as_str().unwrap_or("")))
                        .small()
                        .color(MUTED),
                );
            });
        });
    decision
}

// ── task board ──────────────────────────────────────────────────────────────

const LANES: [(&str, &str, Color32); 5] = [
    ("in_progress", "進行中", BLUE),
    ("awaiting_operator", "あなたの判断待ち", AMBER),
    ("waiting", "待機", VIOLET),
    ("on_hold", "保留", MUTED),
    ("done", "完了", GREEN),
];

const CARD_W: f32 = 236.0;
const CARD_H: f32 = 92.0;

fn lane_of(task: &Value) -> &str {
    match task["status"].as_str().unwrap_or("") {
        "failed" => "done",
        other => other,
    }
}

fn kind_tile(kind: &str) -> (&'static str, Color32) {
    match kind {
        "dialogue" => ("話", Color32::from_rgb(49, 129, 235)),
        "tool" => ("具", Color32::from_rgb(140, 110, 240)),
        "approval" => ("承", AMBER),
        "commit" => ("反", GREEN),
        "job" => ("定", Color32::from_rgb(80, 180, 200)),
        "manual" => ("手", Color32::from_rgb(200, 150, 90)),
        _ => ("澪", Color32::from_rgb(230, 120, 170)),
    }
}

fn node_color(node: &str) -> Color32 {
    if node == "pi" {
        Color32::from_rgb(120, 200, 150)
    } else {
        Color32::from_rgb(120, 170, 240)
    }
}

impl RemoteApp {
    fn task(&self, id: &str) -> Option<&Value> {
        self.tasks.iter().find(|t| t["id"].as_str() == Some(id))
    }

    fn successors(&self, id: &str) -> Vec<&Value> {
        self.tasks
            .iter()
            .filter(|t| {
                t["depends_on"]
                    .as_array()
                    .is_some_and(|d| d.iter().any(|x| x.as_str() == Some(id)))
            })
            .collect()
    }

    fn tasks_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("タスク").color(TEXT));
            let running = self
                .tasks
                .iter()
                .filter(|t| t["status"] == "in_progress")
                .count();
            ui.label(
                RichText::new(format!(
                    "並列で進行中 {running} 件 · 全 {} 件",
                    self.tasks.len()
                ))
                .small()
                .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(
                    &mut self.show_done,
                    RichText::new("完了を表示").small().color(MUTED),
                );
                ui.add_space(10.0);
                ui.selectable_value(&mut self.map_mode, true, "マップ");
                ui.selectable_value(&mut self.map_mode, false, "ボード");
            });
        });
        ui.label(
            RichText::new("対話・ツール呼び出し（llm_master 転送含む）・承認・反映・定期ジョブが自動で載ります。ボードはカードを選ぶと前後がつながり、マップは全体の前提条件 → 後続と承認待ち（琥珀色）を表示します。")
                .small()
                .color(MUTED),
        );
        ui.separator();
        // New task composer.
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_task_title)
                    .hint_text("新しいタスク（澪にも見えます）")
                    .desired_width(360.0),
            );
            if self.selected_task.is_some() {
                ui.checkbox(
                    &mut self.new_task_after_selected,
                    RichText::new("選択中の後に続ける").small().color(MUTED),
                );
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("＋ タスク追加").color(Color32::WHITE))
                        .fill(BLUE),
                )
                .clicked()
                && !self.new_task_title.trim().is_empty()
            {
                let depends: Vec<String> = if self.new_task_after_selected {
                    self.selected_task.clone().into_iter().collect()
                } else {
                    Vec::new()
                };
                let _ = self.commands.send(RemoteCommand::Task(serde_json::json!({
                    "action": "create", "title": self.new_task_title.trim(),
                    "status": "waiting", "depends_on": depends})));
                self.new_task_title.clear();
            }
        });
        ui.add_space(6.0);

        let detail_w = if self.selected_task.is_some() {
            330.0
        } else {
            0.0
        };
        let board_w = (ui.available_width() - detail_w - 12.0).max(300.0);
        let mut clicked: Option<String> = None;
        let top_down = egui::Layout::top_down(egui::Align::Min);
        ui.horizontal_top(|ui| {
            // Explicit top-down: the row layout must not leak into the board.
            ui.allocate_ui_with_layout(
                egui::vec2(board_w, ui.available_height()),
                top_down,
                |ui| {
                    if self.map_mode {
                        self.task_map(ui, &mut clicked);
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("task-board")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut rects: std::collections::HashMap<String, egui::Rect> =
                                std::collections::HashMap::new();
                            let mut numbers: std::collections::HashMap<String, usize> =
                                std::collections::HashMap::new();
                            for (key, label, color) in LANES {
                                if key == "done" && !self.show_done {
                                    continue;
                                }
                                let mut lane: Vec<&Value> =
                                    self.tasks.iter().filter(|t| lane_of(t) == key).collect();
                                lane.sort_by(|a, b| {
                                    a["created_at"].as_u64().cmp(&b["created_at"].as_u64())
                                });
                                if key == "done" {
                                    lane.reverse();
                                    lane.truncate(40);
                                }
                                egui::Frame::new()
                                    .fill(PANEL)
                                    .stroke(Stroke::new(1.0, BORDER))
                                    .corner_radius(egui::CornerRadius::same(12))
                                    .inner_margin(10.0)
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            let (r, _) = ui.allocate_exact_size(
                                                egui::vec2(5.0, 20.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(r, 2.0, color);
                                            ui.label(
                                                RichText::new(label)
                                                    .strong()
                                                    .size(16.0)
                                                    .color(TEXT),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    chip(ui, &format!("{}件", lane.len()), color);
                                                },
                                            );
                                        });
                                        ui.add_space(6.0);
                                        if lane.is_empty() {
                                            ui.label(RichText::new("なし").small().color(MUTED));
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                                            for (i, t) in lane.iter().enumerate() {
                                                let id = t["id"].as_str().unwrap_or("").to_owned();
                                                numbers.insert(id.clone(), i + 1);
                                                let (rect, response) = self.task_card(ui, t, i + 1);
                                                if response.clicked() {
                                                    clicked = Some(id.clone());
                                                }
                                                rects.insert(id, rect);
                                            }
                                        });
                                    });
                                ui.add_space(10.0);
                            }
                            // Dependency links: faint for all, bright around the selection.
                            let painter = ui.painter();
                            let selected = self.selected_task.clone().unwrap_or_default();
                            for t in &self.tasks {
                                let Some(to) = t["id"].as_str().and_then(|id| rects.get(id)) else {
                                    continue;
                                };
                                for dep in t["depends_on"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                                {
                                    let Some(from) = rects.get(dep) else { continue };
                                    let hot = dep == selected
                                        || t["id"].as_str() == Some(selected.as_str());
                                    // The board shows only the selection's links;
                                    // the map view draws the whole graph.
                                    if !hot {
                                        continue;
                                    }
                                    let a = egui::pos2(from.right(), from.center().y);
                                    let b = egui::pos2(to.left(), to.center().y);
                                    let (a, b) = if b.x > a.x + 20.0 {
                                        (a, b)
                                    } else {
                                        (
                                            egui::pos2(from.center().x, from.bottom()),
                                            egui::pos2(to.center().x, to.top()),
                                        )
                                    };
                                    let dx = ((b.x - a.x).abs() * 0.5).max(40.0);
                                    let (c1, c2) = if (a.y - from.bottom()).abs() < 1.0 {
                                        (egui::pos2(a.x, a.y + dx), egui::pos2(b.x, b.y - dx))
                                    } else {
                                        (egui::pos2(a.x + dx, a.y), egui::pos2(b.x - dx, b.y))
                                    };
                                    let stroke = if hot {
                                        Stroke::new(2.4, AMBER)
                                    } else {
                                        Stroke::new(
                                            1.2,
                                            Color32::from_rgba_unmultiplied(140, 160, 190, 90),
                                        )
                                    };
                                    painter.add(
                                        egui::epaint::CubicBezierShape::from_points_stroke(
                                            [a, c1, c2, b],
                                            false,
                                            Color32::TRANSPARENT,
                                            stroke,
                                        ),
                                    );
                                    painter.circle_filled(
                                        b,
                                        if hot { 3.5 } else { 2.5 },
                                        stroke.color,
                                    );
                                }
                            }
                        });
                },
            );
            if let Some(id) = self.selected_task.clone() {
                ui.allocate_ui_with_layout(
                    egui::vec2(detail_w, ui.available_height()),
                    top_down,
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("task-detail")
                            .show(ui, |ui| self.task_detail(ui, &id));
                    },
                );
            }
        });
        if let Some(id) = clicked {
            self.selected_task = if self.selected_task.as_deref() == Some(id.as_str()) {
                None
            } else {
                Some(id)
            };
            self.task_note.clear();
        }
    }

    fn task_card(
        &self,
        ui: &mut egui::Ui,
        t: &Value,
        number: usize,
    ) -> (egui::Rect, egui::Response) {
        let id = t["id"].as_str().unwrap_or("");
        let selected = self.selected_task.as_deref() == Some(id);
        let linked = self.selected_task.as_deref().is_some_and(|sel| {
            t["depends_on"]
                .as_array()
                .is_some_and(|d| d.iter().any(|x| x.as_str() == Some(sel)))
                || self.task(sel).is_some_and(|s| {
                    s["depends_on"]
                        .as_array()
                        .is_some_and(|d| d.iter().any(|x| x.as_str() == Some(id)))
                })
        });
        let failed = t["status"] == "failed";
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(CARD_W, CARD_H), egui::Sense::click());
        let painter = ui.painter_at(rect.expand(2.0));
        let border = if selected {
            Stroke::new(2.0, BLUE)
        } else if linked {
            Stroke::new(1.6, AMBER)
        } else if failed {
            Stroke::new(1.0, RED)
        } else if response.hovered() {
            Stroke::new(1.0, Color32::from_rgb(80, 110, 150))
        } else {
            Stroke::new(1.0, BORDER)
        };
        painter.rect(rect, 10.0, PANEL_RAISED, border, egui::StrokeKind::Inside);
        let (glyph, tile_color) = kind_tile(t["kind"].as_str().unwrap_or(""));
        let tile =
            egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 12.0), egui::vec2(38.0, 38.0));
        painter.rect_filled(tile, 8.0, tile_color.gamma_multiply(0.25));
        painter.text(
            tile.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(18.0),
            tile_color,
        );
        // Title (wrapped, up to three lines).
        let title = t["title"].as_str().unwrap_or("");
        let galley = ui.painter().layout(
            title.to_owned(),
            egui::FontId::proportional(13.0),
            if failed {
                Color32::from_rgb(255, 190, 196)
            } else {
                TEXT
            },
            CARD_W - 90.0,
        );
        let clip = egui::Rect::from_min_size(
            rect.min + egui::vec2(58.0, 10.0),
            egui::vec2(CARD_W - 90.0, 54.0),
        );
        ui.painter_at(clip).galley(clip.min, galley, TEXT);
        // Number badge.
        painter.text(
            rect.right_top() + egui::vec2(-12.0, 10.0),
            egui::Align2::RIGHT_TOP,
            number.to_string(),
            egui::FontId::proportional(11.0),
            MUTED,
        );
        // Footer: node, owner, age.
        let node = t["node"].as_str().unwrap_or("?");
        let footer_y = rect.bottom() - 16.0;
        painter.text(
            egui::pos2(rect.left() + 10.0, footer_y),
            egui::Align2::LEFT_CENTER,
            format!("● {node}"),
            egui::FontId::proportional(11.0),
            node_color(node),
        );
        let owner = match t["owner"].as_str().unwrap_or("") {
            "operator" => "あなた",
            "mio" => "澪",
            _ => "system",
        };
        let when = t["duration_secs"].as_u64().map_or_else(
            || {
                t["updated"]
                    .as_str()
                    .map(|u| u[11..16].to_owned())
                    .unwrap_or_default()
            },
            |d| format!("{d}s"),
        );
        painter.text(
            egui::pos2(rect.right() - 10.0, footer_y),
            egui::Align2::RIGHT_CENTER,
            format!("{owner} · {when}"),
            egui::FontId::proportional(11.0),
            MUTED,
        );
        if failed {
            painter.text(
                egui::pos2(rect.left() + 90.0, footer_y),
                egui::Align2::LEFT_CENTER,
                "失敗",
                egui::FontId::proportional(11.0),
                RED,
            );
        }
        (rect, response.on_hover_text(title))
    }

    fn task_detail(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(t) = self.task(id).cloned() else {
            ui.label(RichText::new("タスクが見つかりません").color(MUTED));
            return;
        };
        let mut goto: Option<String> = None;
        let mut close = false;
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (glyph, color) = kind_tile(t["kind"].as_str().unwrap_or(""));
                    ui.label(RichText::new(glyph).size(22.0).color(color));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.small_button("閉じる").clicked() {
                            close = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(t["title"].as_str().unwrap_or(""))
                        .strong()
                        .size(16.0)
                        .color(TEXT),
                );
                ui.add_space(6.0);
                let status_label = match t["status"].as_str().unwrap_or("") {
                    "in_progress" => "進行中",
                    "awaiting_operator" => "あなたの判断待ち",
                    "waiting" => "待機",
                    "on_hold" => "保留",
                    "done" => "完了",
                    "failed" => "失敗",
                    other => other,
                };
                egui::Grid::new(("task-kv", id))
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for (k, v) in [
                            ("状態", status_label.to_owned()),
                            ("種類", t["kind"].as_str().unwrap_or("").to_owned()),
                            ("ノード", t["node"].as_str().unwrap_or("").to_owned()),
                            ("担当", t["owner"].as_str().unwrap_or("").to_owned()),
                            (
                                "作成",
                                t["created"].as_str().unwrap_or("").replace('T', " "),
                            ),
                            (
                                "更新",
                                t["updated"].as_str().unwrap_or("").replace('T', " "),
                            ),
                        ] {
                            ui.label(RichText::new(k).small().color(MUTED));
                            ui.label(RichText::new(v).small().color(TEXT));
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.label(RichText::new("前タスク").small().strong().color(MUTED));
                let deps: Vec<String> = t["depends_on"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect();
                if deps.is_empty() {
                    ui.label(RichText::new("なし").small().color(MUTED));
                }
                for dep in deps {
                    let label = self
                        .task(&dep)
                        .and_then(|d| d["title"].as_str())
                        .unwrap_or(&dep)
                        .to_owned();
                    if ui
                        .link(RichText::new(format!("← {label}")).small())
                        .clicked()
                    {
                        goto = Some(dep);
                    }
                }
                ui.add_space(4.0);
                ui.label(RichText::new("次タスク").small().strong().color(MUTED));
                let next: Vec<(String, String)> = self
                    .successors(id)
                    .iter()
                    .map(|s| {
                        (
                            s["id"].as_str().unwrap_or("").to_owned(),
                            s["title"].as_str().unwrap_or("").to_owned(),
                        )
                    })
                    .collect();
                if next.is_empty() {
                    ui.label(RichText::new("なし").small().color(MUTED));
                }
                for (nid, title) in next {
                    if ui
                        .link(RichText::new(format!("→ {title}")).small())
                        .clicked()
                    {
                        goto = Some(nid);
                    }
                }
                if t["kind"] == "approval"
                    && t["status"] == "awaiting_operator"
                    && let Some(aid) = t["detail"]["approval"].as_str()
                {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("承認して実行").color(Color32::BLACK),
                                )
                                .fill(GREEN),
                            )
                            .clicked()
                        {
                            let _ = self.commands.send(RemoteCommand::Decide {
                                id: aid.to_owned(),
                                approve: true,
                            });
                        }
                        if ui.button("却下").clicked() {
                            let _ = self.commands.send(RemoteCommand::Decide {
                                id: aid.to_owned(),
                                approve: false,
                            });
                        }
                    });
                }
                ui.add_space(8.0);
                ui.label(RichText::new("メモ・経過").small().strong().color(MUTED));
                for n in t["notes"].as_array().into_iter().flatten().rev() {
                    ui.label(
                        RichText::new(format!(
                            "[{}] {}",
                            n["by"].as_str().unwrap_or(""),
                            n["text"].as_str().unwrap_or("")
                        ))
                        .small()
                        .color(TEXT),
                    );
                }
                let editable = matches!(t["kind"].as_str(), Some("manual" | "agent"));
                if editable {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        for (st, label) in [
                            ("in_progress", "進行中"),
                            ("waiting", "待機"),
                            ("on_hold", "保留"),
                            ("done", "完了"),
                        ] {
                            if ui.small_button(label).clicked() {
                                let _ =
                                    self.commands.send(RemoteCommand::Task(serde_json::json!({
                                    "action": "update", "id": id, "status": st})));
                            }
                        }
                    });
                }
                ui.add(
                    egui::TextEdit::multiline(&mut self.task_note)
                        .hint_text("メモを追加")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("メモを貼る").clicked() && !self.task_note.trim().is_empty() {
                    let _ = self.commands.send(RemoteCommand::Task(serde_json::json!({
                        "action": "update", "id": id, "note": self.task_note.trim()})));
                    self.task_note.clear();
                }
                if !editable {
                    ui.label(
                        RichText::new("自動記録されたタスクは状態を変更できません（メモは可）")
                            .small()
                            .color(MUTED),
                    );
                }
            });
        if close {
            self.selected_task = None;
        }
        if let Some(g) = goto {
            self.selected_task = Some(g);
        }
    }
}

// ── task map (mind-map style) ─────────────────────────────────────────────

const NODE_W: f32 = 210.0;
const NODE_H: f32 = 58.0;
const COL_GAP: f32 = 70.0;
const ROW_GAP: f32 = 14.0;
const GROUP_GAP: f32 = 34.0;

fn status_color(status: &str) -> Color32 {
    match status {
        "in_progress" => BLUE,
        "awaiting_operator" => AMBER,
        "waiting" => VIOLET,
        "on_hold" => MUTED,
        "failed" => RED,
        _ => GREEN,
    }
}

/// Longest prerequisite chain below `id` (cycle-safe).
fn chain_depth<'a>(
    id: &'a str,
    deps: &std::collections::HashMap<&'a str, Vec<&'a str>>,
    memo: &mut std::collections::HashMap<&'a str, usize>,
    stack: &mut std::collections::HashSet<&'a str>,
) -> usize {
    if let Some(d) = memo.get(id) {
        return *d;
    }
    if !stack.insert(id) {
        return 0;
    }
    let d = deps.get(id).map_or(0, |ds| {
        ds.iter()
            .map(|p| chain_depth(p, deps, memo, stack) + 1)
            .max()
            .unwrap_or(0)
    });
    stack.remove(id);
    memo.insert(id, d);
    d
}

fn root_of<'a>(parent: &mut std::collections::HashMap<&'a str, &'a str>, x: &'a str) -> &'a str {
    let mut r = x;
    while parent[r] != r {
        r = parent[r];
    }
    parent.insert(x, r);
    r
}

/// Layered layout: each connected group of tasks becomes one block;
/// prerequisites sit left of their successors (column = longest
/// prerequisite chain) and blocks stack top to bottom, most recent first.
fn map_layout(tasks: &[&Value]) -> (Vec<(String, egui::Pos2)>, egui::Vec2) {
    use std::collections::{HashMap, HashSet};
    let ids: HashSet<&str> = tasks.iter().filter_map(|t| t["id"].as_str()).collect();
    let deps: HashMap<&str, Vec<&str>> = tasks
        .iter()
        .filter_map(|t| {
            let id = t["id"].as_str()?;
            let d = t["depends_on"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|d| ids.contains(d))
                .collect();
            Some((id, d))
        })
        .collect();
    let mut memo = HashMap::new();
    for id in &ids {
        chain_depth(id, &deps, &mut memo, &mut HashSet::new());
    }
    let mut parent: HashMap<&str, &str> = ids.iter().map(|i| (*i, *i)).collect();
    for (id, ds) in &deps {
        for d in ds {
            let (a, b) = (root_of(&mut parent, id), root_of(&mut parent, d));
            if a != b {
                parent.insert(a, b);
            }
        }
    }
    let mut groups: HashMap<&str, Vec<&Value>> = HashMap::new();
    for t in tasks {
        if let Some(id) = t["id"].as_str() {
            let root = root_of(&mut parent, id);
            groups.entry(root).or_default().push(t);
        }
    }
    let mut groups: Vec<Vec<&Value>> = groups.into_values().collect();
    groups.sort_by_key(|g| {
        std::cmp::Reverse(
            g.iter()
                .filter_map(|t| t["updated_at"].as_u64())
                .max()
                .unwrap_or(0),
        )
    });
    let mut out = Vec::new();
    let (mut y, mut max_x) = (10.0_f32, 0.0_f32);
    for mut members in groups {
        members.sort_by_key(|t| t["created_at"].as_u64().unwrap_or(0));
        let mut rows: HashMap<usize, usize> = HashMap::new();
        let mut rows_used = 0usize;
        for t in members {
            let id = t["id"].as_str().unwrap_or("");
            let col = memo.get(id).copied().unwrap_or(0);
            let row = rows.entry(col).or_insert(0);
            let x = 10.0 + col as f32 * (NODE_W + COL_GAP);
            out.push((
                id.to_owned(),
                egui::pos2(x, y + *row as f32 * (NODE_H + ROW_GAP)),
            ));
            *row += 1;
            rows_used = rows_used.max(*row);
            max_x = max_x.max(x + NODE_W);
        }
        y += rows_used as f32 * (NODE_H + ROW_GAP) + GROUP_GAP;
    }
    (out, egui::vec2(max_x + 20.0, y + 10.0))
}

impl RemoteApp {
    fn task_map(&self, ui: &mut egui::Ui, clicked: &mut Option<String>) {
        let visible: Vec<&Value> = self
            .tasks
            .iter()
            .filter(|t| self.show_done || !matches!(t["status"].as_str(), Some("done" | "failed")))
            .collect();
        if visible.is_empty() {
            ui.label(RichText::new("表示するタスクはありません").color(MUTED));
            return;
        }
        let (positions, size) = map_layout(&visible);
        let by_id: std::collections::HashMap<&str, &Value> = visible
            .iter()
            .filter_map(|t| t["id"].as_str().map(|id| (id, *t)))
            .collect();
        egui::ScrollArea::both()
            .id_salt("task-map")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (canvas, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let origin = canvas.min.to_vec2();
                let rect_of: std::collections::HashMap<&str, egui::Rect> = positions
                    .iter()
                    .map(|(id, p)| {
                        (
                            id.as_str(),
                            egui::Rect::from_min_size(*p + origin, egui::vec2(NODE_W, NODE_H)),
                        )
                    })
                    .collect();
                let painter = ui.painter().clone();
                let selected = self.selected_task.as_deref().unwrap_or("");
                // Edges first: prerequisite → successor.
                for (id, to) in &rect_of {
                    let Some(t) = by_id.get(id) else { continue };
                    for dep in t["depends_on"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        let Some(from) = rect_of.get(dep) else {
                            continue;
                        };
                        let hot = dep == selected || *id == selected;
                        let a = egui::pos2(from.right(), from.center().y);
                        let b = egui::pos2(to.left(), to.center().y);
                        let dx = ((b.x - a.x) * 0.5).max(30.0);
                        let color = if hot {
                            AMBER
                        } else {
                            Color32::from_rgba_unmultiplied(150, 170, 200, 120)
                        };
                        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                            [a, egui::pos2(a.x + dx, a.y), egui::pos2(b.x - dx, b.y), b],
                            false,
                            Color32::TRANSPARENT,
                            Stroke::new(if hot { 2.4 } else { 1.4 }, color),
                        ));
                        painter.circle_filled(b, 3.0, color);
                    }
                }
                // Nodes.
                for (id, rect) in &rect_of {
                    let Some(t) = by_id.get(id) else { continue };
                    let status = t["status"].as_str().unwrap_or("");
                    let color = status_color(status);
                    let response = ui.interact(
                        *rect,
                        egui::Id::new(("map-node", *id)),
                        egui::Sense::click(),
                    );
                    if status == "awaiting_operator" {
                        // Where work is blocked on you: glow around the node.
                        painter.rect_filled(rect.expand(6.0), 14.0, AMBER.gamma_multiply(0.28));
                    }
                    let stroke = if *id == selected {
                        Stroke::new(2.2, Color32::WHITE)
                    } else if response.hovered() {
                        Stroke::new(1.6, color)
                    } else {
                        Stroke::new(1.0, color.gamma_multiply(0.7))
                    };
                    painter.rect(*rect, 10.0, PANEL_RAISED, stroke, egui::StrokeKind::Inside);
                    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(5.0, NODE_H));
                    painter.rect_filled(
                        bar,
                        egui::CornerRadius {
                            nw: 10,
                            sw: 10,
                            ne: 0,
                            se: 0,
                        },
                        color,
                    );
                    let (glyph, tile) = kind_tile(t["kind"].as_str().unwrap_or(""));
                    painter.text(
                        rect.min + egui::vec2(20.0, NODE_H / 2.0),
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        egui::FontId::proportional(16.0),
                        tile,
                    );
                    let galley = painter.layout(
                        t["title"].as_str().unwrap_or("").to_owned(),
                        egui::FontId::proportional(12.0),
                        TEXT,
                        NODE_W - 44.0,
                    );
                    let clip = egui::Rect::from_min_size(
                        rect.min + egui::vec2(34.0, 6.0),
                        egui::vec2(NODE_W - 42.0, 32.0),
                    );
                    ui.painter_at(clip).galley(clip.min, galley, TEXT);
                    let status_ja = match status {
                        "in_progress" => "進行中",
                        "awaiting_operator" => "承認待ち",
                        "waiting" => "待機",
                        "on_hold" => "保留",
                        "failed" => "失敗",
                        _ => "完了",
                    };
                    painter.text(
                        egui::pos2(rect.left() + 34.0, rect.bottom() - 11.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{status_ja} · {}", t["node"].as_str().unwrap_or("")),
                        egui::FontId::proportional(10.5),
                        color,
                    );
                    if response.clicked() {
                        *clicked = Some((*id).to_owned());
                    }
                    response.on_hover_text(t["title"].as_str().unwrap_or(""));
                }
            });
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prerequisites_sit_left_of_successors() {
        let a = json!({"id": "a", "created_at": 1, "updated_at": 1, "depends_on": []});
        let b = json!({"id": "b", "created_at": 2, "updated_at": 2, "depends_on": ["a"]});
        let c = json!({"id": "c", "created_at": 3, "updated_at": 3, "depends_on": ["b", "a"]});
        let lone = json!({"id": "z", "created_at": 4, "updated_at": 9, "depends_on": []});
        let tasks = [&a, &b, &c, &lone];
        let (pos, size) = map_layout(&tasks);
        let x = |id: &str| pos.iter().find(|(i, _)| i == id).expect("placed").1.x;
        let y = |id: &str| pos.iter().find(|(i, _)| i == id).expect("placed").1.y;
        assert!(
            x("a") < x("b") && x("b") < x("c"),
            "chain goes left to right"
        );
        assert!(y("z") < y("a"), "most recently updated group is on top");
        assert!(size.x >= x("c") + NODE_W);
    }

    #[test]
    fn cycles_do_not_hang() {
        let a = json!({"id": "a", "depends_on": ["b"]});
        let b = json!({"id": "b", "depends_on": ["a"]});
        let (pos, _) = map_layout(&[&a, &b]);
        assert_eq!(pos.len(), 2);
    }
}
