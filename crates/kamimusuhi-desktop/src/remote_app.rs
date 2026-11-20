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
use crate::task_panel::AgentsPanel;
use kamimusuhi_resident::remote::{RemoteCommand, RemoteEvent};

pub(crate) const RED: Color32 = Color32::from_rgb(235, 96, 110);
pub(crate) const VIOLET: Color32 = Color32::from_rgb(166, 138, 255);
/// The individual accepts at most this much per turn (runtime contract).
const MAX_INPUT_BYTES: usize = kamimusuhi_runtime::dialogue::MAX_INPUT_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Tasks,
    Agents,
    Approvals,
    Memory,
    Status,
    Tools,
}

#[derive(Debug, Clone)]
enum Entry {
    User {
        text: String,
        at: Option<String>,
        subject: Option<String>,
    },
    Assistant {
        text: String,
        meta: Option<Value>,
    },
    Notice {
        text: String,
        color: Color32,
    },
}

pub fn run(
    commands: Sender<RemoteCommand>,
    events: Receiver<RemoteEvent>,
    subject: String,
    initial_view: Option<&str>,
) -> Result<(), String> {
    let view = match initial_view {
        Some("tasks" | "map") => View::Tasks,
        Some("agents") => View::Agents,
        Some("approvals") => View::Approvals,
        Some("memory" | "self") => View::Memory,
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
    subjects: Vec<String>,
    subject_draft: String,
    history_subject: Option<String>,
    url: Option<String>,
    view: View,
    entries: Vec<Entry>,
    input: String,
    chat_filter: String,
    scroll_chat_bottom: bool,
    busy_since: Option<Instant>,
    error: Option<String>,
    status: Option<Value>,
    status_at: Option<Instant>,
    approvals: Vec<Value>,
    announced: HashSet<String>,
    /// Decisions (commitments) and ended delegated tasks already announced;
    /// `None` until the first listing, which is taken as already known.
    attention: Option<HashSet<String>>,
    ended: Option<HashSet<String>>,
    deciding: HashSet<String>,
    last_decisions: Vec<(String, Value)>,
    tools: Option<Value>,
    tool_filter: String,
    tasks: Vec<Value>,
    task_filter: String,
    task_kind_filter: String,
    selected_task: Option<String>,
    show_done: bool,
    new_task_title: String,
    new_task_after_selected: bool,
    task_note: String,
    map_mode: bool,
    agents: AgentsPanel,
    memory: Option<Value>,
    self_notes: Option<Value>,
    memory_domain: &'static str,
    memory_filter: String,
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
            subjects: Vec::new(),
            subject_draft: String::new(),
            history_subject: None,
            url: None,
            view: View::Chat,
            entries: Vec::new(),
            input: String::new(),
            chat_filter: String::new(),
            scroll_chat_bottom: false,
            busy_since: None,
            error: None,
            status: None,
            status_at: None,
            approvals: Vec::new(),
            announced: HashSet::new(),
            attention: None,
            ended: None,
            deciding: HashSet::new(),
            last_decisions: Vec::new(),
            tools: None,
            tool_filter: String::new(),
            tasks: Vec::new(),
            task_filter: String::new(),
            task_kind_filter: "all".to_owned(),
            selected_task: None,
            show_done: true,
            new_task_title: String::new(),
            new_task_after_selected: false,
            task_note: String::new(),
            map_mode: false,
            agents: AgentsPanel::default(),
            memory: None,
            self_notes: None,
            memory_domain: "all",
            memory_filter: String::new(),
        }
    }

    fn pending(&self) -> Vec<&Value> {
        self.approvals
            .iter()
            .filter(|a| a["state"] == "pending")
            .collect()
    }

    fn pending_proposals_count(&self) -> usize {
        self.memory
            .as_ref()
            .and_then(|m| m["pending_proposals"].as_array())
            .map_or(0, Vec::len)
    }

    fn load_history_entries(&mut self, history: &[Value]) {
        self.entries.clear();
        for turn in history {
            let subj = turn["subject"].as_str().map(str::to_owned);
            self.entries.push(Entry::User {
                text: turn["message"].as_str().unwrap_or("").to_owned(),
                at: turn["ts"].as_str().map(str::to_owned),
                subject: subj,
            });
            let response = turn["response"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    turn["error"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|e| format!("⚠ {e}"))
                })
                .unwrap_or_default();
            if !response.is_empty() {
                self.entries.push(Entry::Assistant {
                    text: response,
                    meta: Some(turn.clone()),
                });
            }
        }
        if !self.entries.is_empty() {
            self.entries.push(Entry::Notice {
                text: "── ここまでが保存済みの会話です ──".to_owned(),
                color: MUTED,
            });
        }
    }

    fn switch_subject(&mut self, next: String) {
        let trimmed = next.trim().to_owned();
        if trimmed.is_empty() || trimmed == self.subject {
            return;
        }
        self.subject = trimmed.clone();
        self.subject_draft.clear();
        self.error = None;
        let _ = self.commands.send(RemoteCommand::SwitchSubject(trimmed));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                RemoteEvent::Connected { url, history } => {
                    self.url = Some(url);
                    self.error = None;
                    self.history_subject = Some(self.subject.clone());
                    self.load_history_entries(&history);
                }
                RemoteEvent::History {
                    subject,
                    subjects,
                    turns,
                } => {
                    self.subjects = subjects;
                    if self.history_subject.as_deref() != Some(subject.as_str()) {
                        self.subject = subject.clone();
                        self.history_subject = Some(subject);
                        self.load_history_entries(&turns);
                    }
                }
                RemoteEvent::Memory { memory, self_notes } => {
                    if let Some(m) = memory {
                        self.memory = Some(m);
                    }
                    if let Some(n) = self_notes {
                        self.self_notes = Some(n);
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
                            os_notify("澪: 承認待ち", &approval_title(a));
                        }
                    }
                    self.approvals = list;
                }
                RemoteEvent::Tools(tools) => self.tools = Some(tools),
                RemoteEvent::Tasks(tasks) => {
                    let ids = tasks
                        .iter()
                        .filter(|t| needs_decision(t))
                        .filter_map(|t| t["id"].as_str());
                    for id in newly_seen(&mut self.attention, ids) {
                        if let Some(t) = tasks.iter().find(|t| t["id"] == id.as_str()) {
                            let text = commitment_line(t);
                            self.entries.push(Entry::Notice {
                                text: format!("⚠ 判断が必要です: {text}"),
                                color: AMBER,
                            });
                            os_notify("澪: 判断が必要です", &text);
                        }
                    }
                    self.tasks = tasks;
                }
                RemoteEvent::Agents(panel) => {
                    let tasks = panel["tasks"].as_array().cloned().unwrap_or_default();
                    let ids = tasks
                        .iter()
                        .filter(|t| delegation_ended(t))
                        .filter_map(|t| t["id"].as_str());
                    for id in newly_seen(&mut self.ended, ids) {
                        if let Some(t) = tasks.iter().find(|t| t["id"] == id.as_str()) {
                            let text = ended_line(t);
                            self.entries.push(Entry::Notice {
                                text: text.clone(),
                                color: GREEN,
                            });
                            os_notify("澪: 委譲タスク", &text);
                        }
                    }
                    self.agents.set_panel(panel);
                }
                RemoteEvent::AgentsReply(reply) => self.agents.set_reply(reply),
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
        if matches!(self.subject.as_str(), "*" | "all") {
            self.error = Some(
                "全 subject 横断表示中は送信できません。具体的な subject を選択してください。"
                    .to_owned(),
            );
            return;
        }
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
            subject: Some(self.subject.clone()),
        });
        self.input.clear();
        self.error = None;
        self.scroll_chat_bottom = true;
        self.busy_since = Some(Instant::now());
        if self.commands.send(RemoteCommand::Send { text }).is_err() {
            self.busy_since = None;
            self.error = Some("通信ワーカーが停止しています。".to_owned());
        }
    }

    /// Start an unbacked promise as a delegated task, or drop it.
    fn commitment_action(&mut self, id: &str, start: bool) {
        let body = if start {
            serde_json::json!({"action": "start", "id": id})
        } else {
            serde_json::json!({"action": "update", "id": id, "status": "cancelled",
                               "note": "操作者が取り消し"})
        };
        let _ = self.commands.send(RemoteCommand::Task(body));
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let mut next_subject: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("澪").strong().size(19.0).color(TEXT));
            ui.label(RichText::new("常駐個体").small().color(MUTED));
            if let Some(s) = &self.status {
                let node = s["node"]["node"].as_str().unwrap_or("?");
                let role = s["node"]["role"].as_str().unwrap_or("");
                chip(ui, &format!("{node} · {role}"), BLUE);
                let primary = s["routing"]["active_primary"].as_str().unwrap_or("なし");
                chip(
                    ui,
                    &format!("経路 {primary}"),
                    if primary == "なし" { AMBER } else { GREEN },
                );
                if let Some(cog) = s.get("cognition").filter(|c| !c.is_null()) {
                    let st = cog["state"].as_str().unwrap_or("disabled");
                    if st != "disabled" {
                        let color = match st {
                            "completed" | "idle" => GREEN,
                            "busy" => AMBER,
                            "failed" => RED,
                            _ => MUTED,
                        };
                        chip(ui, &format!("思考 {st}"), color);
                    }
                }
                if let Some(day_usd) = s["routing"]["cost_guard"]["daily_usd"].as_f64() {
                    chip(ui, &format!("今日 ${day_usd:.3}"), MUTED);
                }
            } else {
                ui.label(RichText::new("接続中…").small().color(MUTED));
            }
            ui.separator();
            ui.label(RichText::new("subject").small().color(MUTED));
            if let Some((badge, color)) = channel_badge(&self.subject) {
                chip(ui, badge, color);
            }
            let selected_label = if self.subject == "*" {
                "* (すべて)".to_owned()
            } else {
                self.subject.clone()
            };
            egui::ComboBox::from_id_salt("subject-select")
                .width(150.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.subject == "*", "* (全subject横断)")
                        .clicked()
                    {
                        next_subject = Some("*".to_owned());
                    }
                    let mut choices: Vec<String> = self.subjects.clone();
                    if self.subject != "*" && !choices.contains(&self.subject) {
                        choices.insert(0, self.subject.clone());
                    }
                    for s in choices {
                        let label = match channel_badge(&s) {
                            Some((ch, _)) => format!("[{ch}] {s}"),
                            None => s.clone(),
                        };
                        if ui.selectable_label(self.subject == s, label).clicked() {
                            next_subject = Some(s);
                        }
                    }
                });
            let draft_resp = ui.add(
                egui::TextEdit::singleline(&mut self.subject_draft)
                    .hint_text("任意の subject")
                    .desired_width(110.0),
            );
            let enter = draft_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.small_button("切替").clicked() || enter) && !self.subject_draft.trim().is_empty()
            {
                next_subject = Some(self.subject_draft.trim().to_owned());
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, color) = match (&self.url, self.busy_since) {
                    (None, _) => ("○ 接続待ち".to_owned(), MUTED),
                    (Some(_), Some(t)) => (format!("考え中 {}s", t.elapsed().as_secs()), AMBER),
                    (Some(_), None) => ("● オンライン".to_owned(), GREEN),
                };
                ui.label(RichText::new(label).small().strong().color(color));
                let pending = self.pending().len();
                if pending > 0
                    && ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("✋ 承認待ち {pending}"))
                                    .small()
                                    .strong()
                                    .color(Color32::BLACK),
                            )
                            .fill(AMBER),
                        )
                        .clicked()
                {
                    self.view = View::Approvals;
                }
                let need_dec = self.tasks.iter().filter(|t| needs_decision(t)).count();
                if need_dec > 0
                    && ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("⚠ 判断待ち {need_dec}"))
                                    .small()
                                    .strong()
                                    .color(AMBER),
                            )
                            .stroke(Stroke::new(1.0, AMBER)),
                        )
                        .clicked()
                {
                    self.view = View::Tasks;
                }
            });
        });
        if let Some(subj) = next_subject {
            self.switch_subject(subj);
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("KAMIMUSUHI").small().strong().color(TEXT));
        ui.label(RichText::new("Pi / llm_master 常駐").small().color(MUTED));
        ui.add_space(14.0);
        let pending = self.pending().len();
        let proposals = self.pending_proposals_count();
        let running_tasks = self
            .tasks
            .iter()
            .filter(|t| t["status"] == "in_progress")
            .count();
        let items: [(String, &str, View, bool); 7] = [
            ("▣  対話".to_owned(), "⌘1", View::Chat, false),
            (
                if running_tasks > 0 {
                    format!("☰  タスク ({running_tasks})")
                } else {
                    "☰  タスク".to_owned()
                },
                "⌘2",
                View::Tasks,
                false,
            ),
            (
                match self.agents.running() {
                    0 => "⇄  外部エージェント".to_owned(),
                    n => format!("⇄  外部エージェント ({n})"),
                },
                "⌘3",
                View::Agents,
                false,
            ),
            (
                if pending > 0 {
                    format!("✋  承認 ({pending})")
                } else {
                    "✋  承認".to_owned()
                },
                "⌘4",
                View::Approvals,
                pending > 0,
            ),
            (
                if proposals > 0 {
                    format!("◈  記憶・自己改善 ({proposals})")
                } else {
                    "◈  記憶・自己改善".to_owned()
                },
                "⌘5",
                View::Memory,
                proposals > 0,
            ),
            ("◌  状態".to_owned(), "⌘6", View::Status, false),
            ("⚒  ツール".to_owned(), "⌘7", View::Tools, false),
        ];
        for (label, key_hint, view, alert) in items {
            let selected = self.view == view;
            let color = if alert {
                AMBER
            } else if selected {
                TEXT
            } else {
                MUTED
            };
            let btn_resp = ui.add_sized(
                [ui.available_width(), 34.0],
                egui::Button::new(RichText::new(format!("{label}   ")).color(color))
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
            );
            ui.painter().text(
                btn_resp.rect.right_center() + egui::vec2(-8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                key_hint,
                egui::FontId::proportional(10.0),
                MUTED,
            );
            if btn_resp.clicked() {
                self.view = view;
            }
        }
        ui.add_space(12.0);
        ui.separator();
        if let Some(s) = &self.status {
            ui.add_space(8.0);
            ui.label(RichText::new("インフラ状態").small().strong().color(MUTED));
            ui.add_space(4.0);
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
            if let Some(cog) = s.get("cognition").filter(|c| !c.is_null())
                && cog["state"].as_str() != Some("disabled")
            {
                health_line(ui, "思考", cog["state"].as_str() != Some("failed"));
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if ui.button("↻ 更新 (⌘R)").clicked() {
                let _ = self.commands.send(RemoteCommand::Refresh);
            }
            if let Some(t) = self.status_at {
                ui.label(
                    RichText::new(format!("{}秒前に取得", t.elapsed().as_secs()))
                        .small()
                        .color(MUTED),
                );
            }
            ui.add(
                egui::Label::new(
                    RichText::new(self.url.clone().unwrap_or_default())
                        .small()
                        .color(MUTED),
                )
                .truncate(),
            );
        });
    }

    // ── views ───────────────────────────────────────────────────────────

    fn chat(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.chat_filter)
                    .hint_text("🔍 会話を検索…")
                    .desired_width(240.0),
            );
            if !self.chat_filter.is_empty() && ui.small_button("✕").clicked() {
                self.chat_filter.clear();
            }
            let needle = self.chat_filter.trim().to_lowercase();
            let turn_count = self
                .entries
                .iter()
                .filter(|e| matches!(e, Entry::User { .. }))
                .count();
            if needle.is_empty() {
                ui.label(
                    RichText::new(format!("{turn_count} ターン"))
                        .small()
                        .color(MUTED),
                );
            } else {
                let matched = self
                    .entries
                    .iter()
                    .filter(|e| entry_matches(e, &needle))
                    .count();
                ui.label(
                    RichText::new(format!("一致 {matched} 件"))
                        .small()
                        .color(BLUE),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⬇ 最新へ").clicked() {
                    self.scroll_chat_bottom = true;
                }
            });
        });
        ui.add_space(4.0);
        let composer = 122.0 + if self.error.is_some() { 48.0 } else { 0.0 };
        let height = (ui.available_height() - composer).max(120.0);
        let width = ui.available_width();
        let mut decisions = Vec::new();
        let mut commitment_actions = Vec::new();
        let needle = self.chat_filter.trim().to_lowercase();
        let scroll_bottom = self.scroll_chat_bottom;
        self.scroll_chat_bottom = false;
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
                    for (idx, entry) in self.entries.iter().enumerate() {
                        if !needle.is_empty() && !entry_matches(entry, &needle) {
                            continue;
                        }
                        match entry {
                            Entry::User { text, at, subject } => bubble(
                                ui,
                                idx,
                                true,
                                text,
                                None,
                                at.as_deref(),
                                subject.as_deref(),
                            ),
                            Entry::Assistant { text, meta } => {
                                let subj = meta.as_ref().and_then(|m| m["subject"].as_str());
                                bubble(ui, idx, false, text, meta.as_ref(), None, subj);
                                if let Some(action) = meta
                                    .as_ref()
                                    .and_then(|m| background_of(m, &self.tasks))
                                    .and_then(|bg| background_strip(ui, &bg))
                                {
                                    commitment_actions.push(action);
                                }
                            }
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
                    // So are promises no task backs, from any surface.
                    for t in self.tasks.iter().filter(|t| needs_decision(t)) {
                        if let Some(action) = commitment_card(ui, t) {
                            commitment_actions.push(action);
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
                    if scroll_bottom {
                        let (_, end_rect) =
                            ui.allocate_space(egui::vec2(ui.available_width(), 1.0));
                        ui.scroll_to_rect(end_rect, Some(egui::Align::BOTTOM));
                    }
                });
        });
        for (id, approve) in decisions {
            self.decide(&id, approve);
        }
        for (id, start) in commitment_actions {
            self.commitment_action(&id, start);
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
                let wildcard = matches!(self.subject.as_str(), "*" | "all");
                let enabled = self.busy_since.is_none() && self.url.is_some() && !wildcard;
                let hint = if wildcard {
                    "全 subject 横断表示中 — 送信するには上のプルダウンから具体的な subject を選択してください"
                } else {
                    "メッセージを入力…  (⌘/Ctrl + Enter で送信)"
                };
                let edit = egui::TextEdit::multiline(&mut self.input)
                    .hint_text(hint)
                    .desired_rows(3)
                    .desired_width((ui.available_width() - 92.0).max(240.0));
                let response = ui.add_enabled(enabled, edit);
                let shortcut = response.has_focus()
                    && ctx.input(|i| {
                        i.key_pressed(egui::Key::Enter) && (i.modifiers.ctrl || i.modifiers.command)
                    });
                ui.horizontal(|ui| {
                    let bytes = self.input.trim().len();
                    let over = bytes > MAX_INPUT_BYTES;
                    ui.label(
                        RichText::new("Enter: 改行 / ⌘+Enter: 送信")
                            .small()
                            .color(MUTED),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{bytes} / {MAX_INPUT_BYTES} B"))
                            .small()
                            .color(if over { RED } else { MUTED }),
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
                        if !self.input.is_empty()
                            && ui
                                .add_enabled(self.busy_since.is_none(), egui::Button::new("クリア"))
                                .clicked()
                        {
                            self.input.clear();
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

    fn memory_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("記憶・自己改善").color(TEXT));
            if let Some(mem) = &self.memory
                && let Some(ttl) = mem["cached_for_secs"].as_u64()
            {
                ui.label(
                    RichText::new(format!("キャッシュ {ttl}s · subject: {}", self.subject))
                        .small()
                        .color(MUTED),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("↻ 記憶を再取得").clicked() {
                    let _ = self.commands.send(RemoteCommand::FetchMemory);
                }
            });
        });
        ui.label(
            RichText::new(
                "個体のエピソード記憶・関係性記憶、C0自己モデル、未承認の改善提案、自己ノート、および自律思考 (Cognition) の状態を表示します。",
            )
            .small()
            .color(MUTED),
        );
        ui.separator();

        // KPI strip
        let mem = self.memory.clone();
        let dom = mem.as_ref().map(|m| &m["domains"]);
        let ep_active = dom.and_then(|d| d["episodic"]["active"].as_u64());
        let ep_total = dom.and_then(|d| d["episodic"]["total"].as_u64());
        let rel_active = dom.and_then(|d| d["relationship"]["active"].as_u64());
        let rel_total = dom.and_then(|d| d["relationship"]["total"].as_u64());
        let self_entries = mem
            .as_ref()
            .and_then(|m| m["self_model"]["fields"].as_object())
            .map_or(0usize, |f| {
                f.values().map(|v| v.as_array().map_or(0, Vec::len)).sum()
            });
        let proposals_n = self.pending_proposals_count();
        let notes_n = self
            .self_notes
            .as_ref()
            .and_then(|n| n["self_notes"].as_array())
            .map_or(0, Vec::len);

        ui.horizontal_wrapped(|ui| {
            kpi_tile(
                ui,
                "ACTIVATION",
                &mem.as_ref()
                    .and_then(|m| m["activation_seq"].as_u64())
                    .map_or_else(|| "—".to_owned(), |n| format!("#{n}")),
                &mem.as_ref()
                    .and_then(|m| {
                        m["individual_id"]
                            .as_str()
                            .or_else(|| m["individual"].as_str())
                    })
                    .map(short_str)
                    .unwrap_or_else(|| "読み込み中".to_owned()),
                BLUE,
            );
            kpi_tile(
                ui,
                "EPISODIC",
                &match (ep_active, ep_total) {
                    (Some(a), Some(t)) => format!("{a}/{t}"),
                    _ => "—".to_owned(),
                },
                "active / total",
                GREEN,
            );
            kpi_tile(
                ui,
                "RELATIONSHIP",
                &match (rel_active, rel_total) {
                    (Some(a), Some(t)) => format!("{a}/{t}"),
                    _ => "—".to_owned(),
                },
                "active / total",
                VIOLET,
            );
            kpi_tile(
                ui,
                "SELF MODEL",
                &if mem.is_some() {
                    self_entries.to_string()
                } else {
                    "—".to_owned()
                },
                "entries",
                TEXT,
            );
            kpi_tile(
                ui,
                "PROPOSALS",
                &if mem.is_some() {
                    proposals_n.to_string()
                } else {
                    "—".to_owned()
                },
                "pending",
                if proposals_n > 0 { AMBER } else { MUTED },
            );
            kpi_tile(
                ui,
                "SELF NOTES",
                &if self.self_notes.is_some() {
                    notes_n.to_string()
                } else {
                    "—".to_owned()
                },
                "recent scratch",
                BLUE,
            );
        });
        ui.add_space(8.0);

        if let Some(s) = &self.status
            && let Some(cog) = s.get("cognition").filter(|c| !c.is_null())
        {
            card(ui, "自律思考 (Idle / API Cognition)", |ui| {
                let st = cog["state"].as_str().unwrap_or("disabled");
                let color = match st {
                    "completed" | "idle" => GREEN,
                    "busy" => AMBER,
                    "failed" => RED,
                    _ => MUTED,
                };
                ui.horizontal_wrapped(|ui| {
                    chip(ui, st, color);
                    if let Some(subj) = cog["subject"].as_str().filter(|x| !x.is_empty()) {
                        chip(ui, &format!("subject: {subj}"), BLUE);
                    }
                    if let Some(ms) = cog["latency_ms"].as_u64() {
                        chip(ui, &format!("{ms} ms"), MUTED);
                    }
                    if let Some(reason) = cog["reason"].as_str().filter(|x| !x.is_empty()) {
                        ui.label(RichText::new(reason).small().color(TEXT));
                    }
                    if let Some(at) = cog["last_run"].as_str().filter(|x| !x.is_empty()) {
                        ui.label(
                            RichText::new(format!("最終実行 {}", fmt_ts(at)))
                                .small()
                                .color(MUTED),
                        );
                    }
                });
            });
            ui.add_space(8.0);
        }

        let Some(mem) = mem else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("記憶レポートを取得中…").small().color(MUTED));
            });
            return;
        };

        egui::ScrollArea::vertical()
            .id_salt("memory-view")
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    // Left: memory records
                    card(&mut cols[0], "記憶レコード", |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (key, label) in [
                                ("all", "すべて"),
                                ("episodic", "episodic"),
                                ("relationship", "relationship"),
                            ] {
                                ui.selectable_value(&mut self.memory_domain, key, label);
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.memory_filter)
                                    .hint_text("🔍 記憶を検索…")
                                    .desired_width(240.0),
                            );
                            if !self.memory_filter.is_empty() && ui.small_button("✕").clicked() {
                                self.memory_filter.clear();
                            }
                        });
                        ui.add_space(6.0);

                        let needle = self.memory_filter.trim().to_lowercase();
                        let mut records: Vec<(&str, &Value)> = Vec::new();
                        for dom_name in ["episodic", "relationship"] {
                            if self.memory_domain != "all" && self.memory_domain != dom_name {
                                continue;
                            }
                            if let Some(arr) = mem["domains"][dom_name]["records"].as_array() {
                                for r in arr {
                                    records.push((dom_name, r));
                                }
                            }
                        }
                        records.sort_by(|a, b| {
                            b.1["created_at"]
                                .as_str()
                                .unwrap_or("")
                                .cmp(a.1["created_at"].as_str().unwrap_or(""))
                        });
                        let mut shown = 0usize;
                        for (dom_name, r) in &records {
                            let body = memory_payload_text(&r["payload"]);
                            let subj_key = r["subject_key"].as_str().unwrap_or("—");
                            let tags_str = r["tags"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                })
                                .unwrap_or_default();
                            if !needle.is_empty()
                                && !body.to_lowercase().contains(&needle)
                                && !subj_key.to_lowercase().contains(&needle)
                                && !tags_str.to_lowercase().contains(&needle)
                            {
                                continue;
                            }
                            shown += 1;
                            egui::Frame::new()
                                .fill(PANEL_RAISED)
                                .stroke(Stroke::new(1.0, BORDER))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal_wrapped(|ui| {
                                        chip(
                                            ui,
                                            dom_name,
                                            if *dom_name == "episodic" {
                                                GREEN
                                            } else {
                                                VIOLET
                                            },
                                        );
                                        chip(ui, subj_key, BLUE);
                                        if let Some((ch, c)) = channel_badge(subj_key) {
                                            chip(ui, ch, c);
                                        }
                                        if let Some(u) = r["use_count"].as_u64() {
                                            ui.label(
                                                RichText::new(format!("×{u}")).small().color(MUTED),
                                            );
                                        }
                                        if let Some(at) = r["created_at"].as_str() {
                                            ui.label(
                                                RichText::new(fmt_ts(at)).small().color(MUTED),
                                            );
                                        }
                                    });
                                    ui.add_space(4.0);
                                    ui.add(
                                        egui::Label::new(RichText::new(&body).small().color(TEXT))
                                            .wrap()
                                            .selectable(true),
                                    );
                                    if let Some(tags) =
                                        r["tags"].as_array().filter(|t| !t.is_empty())
                                    {
                                        ui.add_space(4.0);
                                        ui.horizontal_wrapped(|ui| {
                                            for t in tags {
                                                if let Some(ts) = t.as_str() {
                                                    chip(ui, ts, MUTED);
                                                }
                                            }
                                        });
                                    }
                                });
                            ui.add_space(6.0);
                        }
                        if shown == 0 {
                            ui.label(
                                RichText::new("一致する記憶レコードはありません。")
                                    .small()
                                    .color(MUTED),
                            );
                        }
                    });

                    // Right: proposals, self_notes, self_model, writeback
                    card(
                        &mut cols[1],
                        "未承認の改善提案 (Pending Proposals)",
                        |ui| {
                            let pending = mem["pending_proposals"]
                                .as_array()
                                .map(Vec::as_slice)
                                .unwrap_or(&[]);
                            if pending.is_empty() {
                                ui.label(
                                    RichText::new("保留中の改善提案はありません。")
                                        .small()
                                        .color(MUTED),
                                );
                            } else {
                                for p in pending {
                                    egui::Frame::new()
                                        .fill(PANEL_RAISED)
                                        .stroke(Stroke::new(1.0, AMBER.gamma_multiply(0.7)))
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(8.0)
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal_wrapped(|ui| {
                                                chip(ui, p["kind"].as_str().unwrap_or("?"), AMBER);
                                                chip(
                                                    ui,
                                                    p["status"].as_str().unwrap_or("?"),
                                                    MUTED,
                                                );
                                                if let Some(pid) = p["proposal_id"].as_str() {
                                                    ui.label(
                                                        RichText::new(pid)
                                                            .small()
                                                            .monospace()
                                                            .color(MUTED),
                                                    );
                                                }
                                            });
                                            ui.add_space(3.0);
                                            let target = p["target"].as_str().unwrap_or("?");
                                            let key = p["target_key"]
                                                .as_str()
                                                .map(|k| format!(".{k}"))
                                                .unwrap_or_default();
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!(
                                                        "{target}{key} → {}",
                                                        p["proposed_value"]
                                                    ))
                                                    .small()
                                                    .color(TEXT),
                                                )
                                                .wrap()
                                                .selectable(true),
                                            );
                                            if let Some(eff) = p["expected_effect"]
                                                .as_str()
                                                .filter(|s| !s.is_empty())
                                            {
                                                ui.label(
                                                    RichText::new(format!("期待効果: {eff}"))
                                                        .small()
                                                        .color(MUTED),
                                                );
                                            }
                                        });
                                    ui.add_space(6.0);
                                }
                            }
                        },
                    );
                    cols[1].add_space(8.0);
                    card(
                        &mut cols[1],
                        "自己ノート (Self Notes · unverified)",
                        |ui| {
                            let notes = self
                                .self_notes
                                .as_ref()
                                .and_then(|n| n["self_notes"].as_array())
                                .map(Vec::as_slice)
                                .unwrap_or(&[]);
                            if notes.is_empty() {
                                ui.label(
                                    RichText::new("self_notes はまだありません。")
                                        .small()
                                        .color(MUTED),
                                );
                            } else {
                                for n in notes.iter().rev() {
                                    egui::Frame::new()
                                        .fill(PANEL_RAISED)
                                        .stroke(Stroke::new(1.0, BORDER))
                                        .corner_radius(egui::CornerRadius::same(8))
                                        .inner_margin(8.0)
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal_wrapped(|ui| {
                                                chip(
                                                    ui,
                                                    n["kind"].as_str().unwrap_or("note"),
                                                    BLUE,
                                                );
                                                let nid = n["note_id"]
                                                    .as_str()
                                                    .map(str::to_owned)
                                                    .unwrap_or_else(|| {
                                                        format!(
                                                            "sn-{}",
                                                            n["seq"].as_u64().unwrap_or(0)
                                                        )
                                                    });
                                                ui.label(
                                                    RichText::new(nid)
                                                        .small()
                                                        .monospace()
                                                        .color(MUTED),
                                                );
                                                if let Some(at) = n["created_at"].as_str() {
                                                    ui.label(
                                                        RichText::new(fmt_ts(at))
                                                            .small()
                                                            .color(MUTED),
                                                    );
                                                }
                                            });
                                            ui.add_space(3.0);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(n["text"].as_str().unwrap_or(""))
                                                        .small()
                                                        .color(TEXT),
                                                )
                                                .wrap()
                                                .selectable(true),
                                            );
                                            if let Some(refs) = n["evidence_refs"]
                                                .as_array()
                                                .filter(|r| !r.is_empty())
                                            {
                                                let joined = refs
                                                    .iter()
                                                    .filter_map(Value::as_str)
                                                    .collect::<Vec<_>>()
                                                    .join(", ");
                                                ui.label(
                                                    RichText::new(format!("refs: {joined}"))
                                                        .small()
                                                        .color(MUTED),
                                                );
                                            }
                                        });
                                    ui.add_space(6.0);
                                }
                            }
                        },
                    );
                    cols[1].add_space(8.0);
                    card(&mut cols[1], "自己モデル / Writeback 健全性", |ui| {
                        let wb = &mem["memory_writeback"];
                        if wb.is_object() {
                            let wb_state = wb["state"].as_str().unwrap_or("?");
                            let ok = wb["unresolved"].as_u64().unwrap_or(0) == 0
                                && wb["aborted"].as_u64().unwrap_or(0) == 0;
                            node_row(
                                ui,
                                "writeback",
                                ok,
                                &format!(
                                    "{wb_state} · unresolved {} · aborted {}",
                                    wb["unresolved"], wb["aborted"]
                                ),
                            );
                        }
                        let proj = &mem["self_projection"];
                        if proj.is_object() {
                            ui.label(
                                RichText::new(format!(
                                    "self_projection: surfaced {}/{} · excluded {}",
                                    proj["surfaced_entries"],
                                    proj["stored_entries"],
                                    proj["excluded_count"]
                                ))
                                .small()
                                .color(MUTED),
                            );
                        }
                        ui.add_space(4.0);
                        if let Some(fields) = mem["self_model"]["fields"].as_object() {
                            for (field, entries) in fields {
                                let arr = entries.as_array().map(Vec::as_slice).unwrap_or(&[]);
                                if arr.is_empty() {
                                    continue;
                                }
                                egui::CollapsingHeader::new(
                                    RichText::new(format!("{field} ({})", arr.len()))
                                        .small()
                                        .color(TEXT),
                                )
                                .id_salt(("self-field", field))
                                .show(ui, |ui| {
                                    for e in arr {
                                        let key = e["key"]
                                            .as_str()
                                            .map(|k| format!("[{k}] "))
                                            .unwrap_or_default();
                                        let conf = e["confidence"]
                                            .as_f64()
                                            .map(|c| format!(" ({c:.2})"))
                                            .unwrap_or_default();
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(format!("{key}{}{conf}", e["value"]))
                                                    .small()
                                                    .color(MUTED),
                                            )
                                            .wrap()
                                            .selectable(true),
                                        );
                                    }
                                });
                            }
                        }
                    });
                });
            });
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
            if let Some(s) = &self.status {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("📋 状態レポートをコピー").clicked() {
                        let (rendered, _) = kamimusuhi_resident::status::render(s);
                        ui.ctx().copy_text(rendered);
                    }
                });
            }
        });
        ui.separator();
        let Some(s) = &self.status else {
            ui.spinner();
            return;
        };

        // Top KPI strip
        let n = &s["node"];
        let ns = &s["node_state"];
        let r = &s["routing"];
        let cg = &r["cost_guard"];
        let proc_m = &s["process"];
        let cog = &s["cognition"];
        ui.horizontal_wrapped(|ui| {
            let uptime = n["uptime_secs"]
                .as_u64()
                .map_or_else(|| "—".to_owned(), fmt_uptime);
            kpi_tile(
                ui,
                "NODE",
                ns["status"].as_str().unwrap_or("—"),
                &format!(
                    "{} · {} · up {uptime}",
                    n["node"].as_str().unwrap_or("?"),
                    ns["mode"]
                        .as_str()
                        .unwrap_or(n["mode"].as_str().unwrap_or("?"))
                ),
                if ns["status"] == "busy" { AMBER } else { GREEN },
            );
            kpi_tile(
                ui,
                "PRIMARY ROUTE",
                r["active_primary"].as_str().unwrap_or("なし"),
                &format!("総リクエスト {}", r["requests"]),
                if r["active_primary"].is_null() {
                    RED
                } else {
                    BLUE
                },
            );
            let day_usd = cg["daily_usd"]
                .as_f64()
                .map_or_else(|| "—".to_owned(), |v| format!("${v:.3}"));
            let month_usd = cg["monthly_usd"]
                .as_f64()
                .map_or_else(|| "—".to_owned(), |v| format!("${v:.2}"));
            kpi_tile(
                ui,
                "COST GUARD",
                &day_usd,
                &format!(
                    "今月 {month_usd} · session {}",
                    fmt_opt_usd(&cg["session_usd"])
                ),
                AMBER,
            );
            let rss = proc_m["rss_bytes"]
                .as_u64()
                .map_or_else(|| "—".to_owned(), |b| format!("{} MiB", b / (1024 * 1024)));
            let cpu = proc_m["cpu_pct_avg"]
                .as_f64()
                .map_or_else(|| "—".to_owned(), |c| format!("{c:.1}%"));
            kpi_tile(
                ui,
                "PROCESS / HOSTS",
                &rss,
                &format!("CPU {cpu} · hosts {}", n["dialogue_hosts"]),
                VIOLET,
            );
            let cog_st = cog["state"].as_str().unwrap_or("disabled");
            kpi_tile(
                ui,
                "COGNITION",
                cog_st,
                cog["subject"].as_str().unwrap_or("—"),
                match cog_st {
                    "completed" | "idle" => GREEN,
                    "busy" => AMBER,
                    "failed" => RED,
                    _ => MUTED,
                },
            );
        });
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .id_salt("status")
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    card(&mut cols[0], "ノード & ピア", |ui| {
                        node_row(
                            ui,
                            n["node"].as_str().unwrap_or("?"),
                            true,
                            &format!(
                                "{} ({}) · 起動 {} · epoch {} · K-CORE {}",
                                n["role"].as_str().unwrap_or(""),
                                n["mode"].as_str().unwrap_or(""),
                                n["uptime_secs"]
                                    .as_u64()
                                    .map_or_else(|| "-".to_owned(), fmt_uptime),
                                n["boot_epoch"],
                                n["kcore"].as_str().unwrap_or("-")
                            ),
                        );
                        if let Some(goal) = ns["active_goal"]["goal"].as_str() {
                            ui.label(
                                RichText::new(format!("稼働目標: {goal}"))
                                    .small()
                                    .color(BLUE),
                            );
                        }
                        if let Some(sess) = ns["active_sessions"]
                            .as_array()
                            .filter(|a| !a.is_empty())
                        {
                            let list: Vec<&str> = sess.iter().filter_map(Value::as_str).collect();
                            ui.label(
                                RichText::new(format!("実行中セッション: {}", list.join(", ")))
                                    .small()
                                    .color(AMBER),
                            );
                        }
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
                                        "{} · {}ms · local LLM {} · {}",
                                        d["role"].as_str().unwrap_or(""),
                                        p["latency_ms"],
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
                    card(&mut cols[0], "ルーティング & Cost Guard", |ui| {
                        let order = r["order"]
                            .as_array()
                            .map(|o| {
                                o.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(" → ")
                            })
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(format!(
                                "Primary: {}  (order: {order})",
                                r["active_primary"].as_str().unwrap_or("なし")
                            ))
                            .color(TEXT),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Cost Guard: session {} (残 {}) · day {} · month {} · unpriced {}",
                                fmt_opt_usd(&cg["session_usd"]),
                                fmt_opt_usd(&cg["session_remaining_usd"]),
                                fmt_opt_usd(&cg["daily_usd"]),
                                fmt_opt_usd(&cg["monthly_usd"]),
                                cg["unpriced_requests"],
                            ))
                            .small()
                            .color(MUTED),
                        );
                        ui.add_space(4.0);
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
                            let lane = last["lane"]
                                .as_str()
                                .map(|l| format!("[{l}] "))
                                .unwrap_or_default();
                            let cost = kamimusuhi_resident::status::route_cost_label(last)
                                .map(|c| format!(" · {c}"))
                                .unwrap_or_default();
                            let cache = kamimusuhi_resident::status::route_cache_label(last)
                                .map(|c| format!(" · {c}"))
                                .unwrap_or_default();
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "直近: {lane}{} {} · {}ms{cost}{cache}",
                                    last["tier"].as_str().unwrap_or("-"),
                                    last["model"].as_str().unwrap_or(""),
                                    last["latency_ms"],
                                ))
                                .small()
                                .color(MUTED),
                            );
                        }
                    });
                    cols[0].add_space(8.0);
                    card(&mut cols[0], "Light / Heavy 委譲 (Delegations)", |ui| {
                        let del = &s["delegations"];
                        if !del["enabled"].as_bool().unwrap_or(false) {
                            ui.label(RichText::new("無効").small().color(MUTED));
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "待機 {} · 完了 {} · 失敗 {} · timeout {} · retry {} · 直近 {}ms",
                                    del["pending"],
                                    del["completed"],
                                    del["failed"],
                                    del["timeouts"],
                                    del["retries"],
                                    del["last_latency_ms"],
                                ))
                                .small()
                                .color(TEXT),
                            );
                            if let Some(err) = del["last_error"].as_str().filter(|e| !e.is_empty()) {
                                ui.label(RichText::new(format!("last_error: {err}")).small().color(RED));
                            }
                            for item in del["recent"].as_array().into_iter().flatten().take(5) {
                                let st = item["state"].as_str().unwrap_or("?");
                                node_row(
                                    ui,
                                    item["task_id"].as_str().unwrap_or("?"),
                                    st == "completed" || st == "running" || st == "pending",
                                    &format!(
                                        "{st} · {} → {}",
                                        item["type"].as_str().unwrap_or("-"),
                                        item["node"].as_str().unwrap_or("-")
                                    ),
                                );
                            }
                        }
                    });

                    // Right column
                    card(&mut cols[1], "Cognition & K-CORE", |ui| {
                        let cog_st = cog["state"].as_str().unwrap_or("disabled");
                        node_row(
                            ui,
                            "Cognition",
                            cog_st != "failed",
                            &format!(
                                "{cog_st} · subject {} · {} · {}",
                                cog["subject"].as_str().unwrap_or("-"),
                                cog["reason"].as_str().unwrap_or("-"),
                                cog["last_run"].as_str().map(fmt_ts).unwrap_or_else(|| "-".to_owned())
                            ),
                        );
                        let kc = &s["kcore"];
                        let kc_st = kc["state"].as_str().unwrap_or("disabled");
                        node_row(
                            ui,
                            "K-CORE",
                            kc_st != "failed",
                            &format!(
                                "{kc_st} · restarts {} · last {}",
                                kc["restarts"],
                                kc["last_output"]["at"]
                                    .as_str()
                                    .map(fmt_ts)
                                    .unwrap_or_else(|| "-".to_owned())
                            ),
                        );
                    });
                    cols[1].add_space(8.0);
                    card(&mut cols[1], "Commitments & Intercom (個体間通信)", |ui| {
                        let cm = &s["commitments"];
                        if cm["enabled"].as_bool().unwrap_or(false) {
                            let tot = &cm["totals"];
                            ui.label(
                                RichText::new(format!(
                                    "Commitments: 計 {} · 試行 {} · 継続 {} · エスカレーション {}",
                                    tot["created"],
                                    tot["attempts"],
                                    tot["resumes"],
                                    tot["escalated_now"],
                                ))
                                .small()
                                .color(TEXT),
                            );
                            for esc in cm["escalated"].as_array().into_iter().flatten() {
                                ui.label(
                                    RichText::new(format!(
                                        "⚠ 確認待ち {}: {}",
                                        esc["id"].as_str().unwrap_or("?"),
                                        esc["question"]
                                            .as_str()
                                            .or_else(|| esc["goal"].as_str())
                                            .unwrap_or("")
                                    ))
                                    .small()
                                    .color(AMBER),
                                );
                            }
                        } else {
                            ui.label(RichText::new("Commitments: 無効").small().color(MUTED));
                        }
                        ui.add_space(4.0);
                        let ic = &s["intercom"];
                        if ic["enabled"].as_bool().unwrap_or(false) {
                            let ob = &ic["outbox"];
                            let convs = ic["conversations"].as_array().map_or(0, Vec::len);
                            ui.label(
                                RichText::new(format!(
                                    "Intercom: inbound {} · outbox queued {} / dead {} · 会話 {convs} 件",
                                    ic["inbound_queued"], ob["queued"], ob["dead"]
                                ))
                                .small()
                                .color(TEXT),
                            );
                        } else {
                            ui.label(RichText::new("Intercom: 無効").small().color(MUTED));
                        }
                    });
                    cols[1].add_space(8.0);
                    card(&mut cols[1], "NAS & HAI", |ui| {
                        let nas = &s["nas"];
                        let warn = nas["spool_warn"].as_bool().unwrap_or(false);
                        node_row(
                            ui,
                            nas["root"].as_str().unwrap_or("NAS (未設定)"),
                            nas["probe"]["healthy"].as_bool().unwrap_or(false),
                            &format!(
                                "未配送 {} files / {} bytes{} · 最終同期 {}",
                                nas["spool_backlog_files"],
                                nas["spool_backlog_bytes"],
                                if warn { " ⚠ LARGE" } else { "" },
                                nas["sync"]["last_success"]
                                    .as_str()
                                    .map(fmt_ts)
                                    .unwrap_or_else(|| "-".to_owned())
                            ),
                        );
                        let hai = &s["routing"]["tiers"]["hai"];
                        let low_bal = s["hai_account"]["detail"]["low_balance"]
                            .as_bool()
                            .unwrap_or(false);
                        node_row(
                            ui,
                            "HAI API",
                            hai["healthy"].as_bool().unwrap_or(false),
                            &format!(
                                "{}ms · 残高 {} JPY{}",
                                hai["latency_ms"],
                                s["hai_account"]["detail"]["balance_jpy"]
                                    .as_str()
                                    .unwrap_or("-"),
                                if low_bal { " ⚠ LOW" } else { "" }
                            ),
                        );
                    });
                    cols[1].add_space(8.0);
                    card(&mut cols[1], "MCP サーバー / 定期ジョブ / Snapshot", |ui| {
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
                        for (name, j) in s["jobs"].as_object().into_iter().flatten() {
                            node_row(
                                ui,
                                name,
                                j["ok"].as_bool().unwrap_or(false),
                                &format!(
                                    "最終 {}",
                                    j["last_run"]
                                        .as_str()
                                        .map(fmt_ts)
                                        .unwrap_or_else(|| "-".to_owned())
                                ),
                            );
                        }
                        let snap = &s["snapshot"];
                        node_row(
                            ui,
                            "snapshot",
                            snap["ok"].as_bool().unwrap_or(false),
                            &snap["at"]
                                .as_str()
                                .map(fmt_ts)
                                .unwrap_or_else(|| "-".to_owned()),
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
                let all_tools = tools["tools"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                let filter = self.tool_filter.trim().to_lowercase();
                let filtered: Vec<&Value> = all_tools
                    .iter()
                    .filter(|t| {
                        if filter.is_empty() {
                            return true;
                        }
                        let f = &t["function"];
                        let name = f["name"].as_str().unwrap_or("").to_lowercase();
                        let desc = f["description"].as_str().unwrap_or("").to_lowercase();
                        name.contains(&filter) || desc.contains(&filter)
                    })
                    .collect();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "tool 一覧 ({}/{})",
                            filtered.len(),
                            all_tools.len()
                        ))
                        .strong()
                        .color(TEXT),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tool_filter)
                            .hint_text("🔍 名前・説明で絞り込み…")
                            .desired_width(240.0),
                    );
                    if !self.tool_filter.is_empty() && ui.small_button("✕").clicked() {
                        self.tool_filter.clear();
                    }
                });
                ui.add_space(4.0);
                for t in filtered {
                    let f = &t["function"];
                    let name = f["name"].as_str().unwrap_or("");
                    let desc = f["description"].as_str().unwrap_or("");
                    let gated = desc.contains("要承認");
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(name).monospace().color(if gated {
                            AMBER
                        } else {
                            VIOLET
                        }));
                        if gated {
                            chip(ui, "要承認", AMBER);
                        }
                        ui.label(
                            RichText::new(desc.chars().take(160).collect::<String>())
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
        ctx.input(|i| {
            let cmd = i.modifiers.ctrl || i.modifiers.command;
            if (cmd && i.key_pressed(egui::Key::Num1)) || i.key_pressed(egui::Key::F1) {
                self.view = View::Chat;
            } else if (cmd && i.key_pressed(egui::Key::Num2)) || i.key_pressed(egui::Key::F2) {
                self.view = View::Tasks;
            } else if (cmd && i.key_pressed(egui::Key::Num3)) || i.key_pressed(egui::Key::F3) {
                self.view = View::Agents;
            } else if (cmd && i.key_pressed(egui::Key::Num4)) || i.key_pressed(egui::Key::F4) {
                self.view = View::Approvals;
            } else if (cmd && i.key_pressed(egui::Key::Num5)) || i.key_pressed(egui::Key::F5) {
                self.view = View::Memory;
            } else if (cmd && i.key_pressed(egui::Key::Num6)) || i.key_pressed(egui::Key::F6) {
                self.view = View::Status;
            } else if (cmd && i.key_pressed(egui::Key::Num7)) || i.key_pressed(egui::Key::F7) {
                self.view = View::Tools;
            } else if cmd && i.key_pressed(egui::Key::R) {
                let _ = self.commands.send(RemoteCommand::Refresh);
            } else if cmd && i.key_pressed(egui::Key::M) && self.view == View::Tasks {
                self.map_mode = !self.map_mode;
            } else if i.key_pressed(egui::Key::Escape) {
                if self.view == View::Tasks && self.selected_task.is_some() {
                    self.selected_task = None;
                } else if self.view == View::Agents && self.agents.has_selection() {
                    self.agents.clear_selection();
                }
            }
        });
        egui::TopBottomPanel::top("remote-top")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(10.0))
            .show(ctx, |ui| self.top_bar(ui));
        egui::SidePanel::left("remote-nav")
            .frame(egui::Frame::new().fill(BG).inner_margin(12.0))
            .resizable(false)
            .default_width(214.0)
            .show(ctx, |ui| self.nav(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(16.0))
            .show(ctx, |ui| match self.view {
                View::Chat => self.chat(ui, ctx),
                View::Tasks => self.tasks_view(ui),
                View::Agents => self.agents.show(ui, &self.commands),
                View::Approvals => self.approvals_view(ui),
                View::Memory => self.memory_view(ui),
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

fn channel_badge(subject: &str) -> Option<(&'static str, Color32)> {
    if matches!(subject, "*" | "all") {
        Some(("横断", VIOLET))
    } else if subject.starts_with("slack-") {
        Some(("Slack", Color32::from_rgb(74, 185, 232)))
    } else if subject.starts_with("discord-") {
        Some(("Discord", Color32::from_rgb(120, 140, 245)))
    } else {
        None
    }
}

fn entry_matches(entry: &Entry, needle: &str) -> bool {
    match entry {
        Entry::User { text, subject, .. } => {
            text.to_lowercase().contains(needle)
                || subject
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(needle))
        }
        Entry::Assistant { text, meta } => {
            if text.to_lowercase().contains(needle) {
                return true;
            }
            meta.as_ref().is_some_and(|m| {
                m["subject"]
                    .as_str()
                    .is_some_and(|s| s.to_lowercase().contains(needle))
                    || m["tool_calls"].as_array().is_some_and(|arr| {
                        arr.iter().any(|t| {
                            t["name"]
                                .as_str()
                                .is_some_and(|n| n.to_lowercase().contains(needle))
                        })
                    })
            })
        }
        Entry::Notice { .. } => false,
    }
}

fn memory_payload_text(payload: &Value) -> String {
    if let Some(s) = payload.as_str() {
        return s.to_owned();
    }
    for key in ["text", "summary", "fact", "description", "content"] {
        if let Some(s) = payload[key].as_str().filter(|v| !v.is_empty()) {
            return s.to_owned();
        }
    }
    serde_json::to_string(payload).unwrap_or_default()
}

fn short_str(s: &str) -> String {
    if s.chars().count() <= 16 {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(15).collect();
        out.push('…');
        out
    }
}

fn fmt_ts(ts: &str) -> String {
    ts.replace('T', " ").trim_end_matches('Z').to_owned()
}

fn fmt_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

fn fmt_opt_usd(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| "—".to_owned(), |x| format!("${x:.3}"))
}

fn kpi_tile(ui: &mut egui::Ui, label: &str, value: &str, note: &str, color: Color32) {
    egui::Frame::new()
        .fill(PANEL_RAISED)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_min_width(136.0);
            ui.label(RichText::new(label).small().color(MUTED));
            ui.label(RichText::new(value).strong().size(18.0).color(color));
            ui.label(RichText::new(note).small().color(MUTED));
        });
}

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
        ui.add(egui::Label::new(RichText::new(detail).small().color(MUTED)).wrap());
    });
}

pub(crate) fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        });
}

fn bubble(
    ui: &mut egui::Ui,
    idx: usize,
    user: bool,
    text: &str,
    meta: Option<&Value>,
    at: Option<&str>,
    subject: Option<&str>,
) {
    let (fill, stroke) = if user {
        (
            Color32::from_rgb(22, 66, 126),
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
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(780.0));
                    // The parent row is left/right aligned; the bubble body
                    // itself must stack top-to-bottom and wrap.
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
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
                            if let Some(subj) = subject.filter(|s| !s.is_empty()) {
                                if let Some((ch, color)) = channel_badge(subj) {
                                    chip(ui, ch, color);
                                } else {
                                    ui.label(RichText::new(subj).small().color(MUTED));
                                }
                            }
                            if let Some(ts) = at.or_else(|| meta.and_then(|m| m["ts"].as_str())) {
                                ui.label(RichText::new(fmt_ts(ts)).small().color(MUTED));
                            }
                            if ui
                                .small_button(RichText::new("📋").small().color(MUTED))
                                .on_hover_text("本文をコピー")
                                .clicked()
                            {
                                ui.ctx().copy_text(text.to_owned());
                            }
                        });
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(RichText::new(text).color(TEXT))
                                .wrap()
                                .selectable(true),
                        );
                        if let Some(meta) = meta {
                            let tools = meta["tool_calls"].as_array().cloned().unwrap_or_default();
                            let tier = meta["tier"].as_str();
                            let route = meta.get("route").filter(|r| !r.is_null());
                            let latency = meta["latency_ms"].as_u64();
                            let lookups = meta["reference_lookups"].as_u64().unwrap_or(0);
                            if tier.is_some()
                                || route.is_some()
                                || latency.is_some()
                                || !tools.is_empty()
                                || lookups > 0
                            {
                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    if let Some(route) = route {
                                        if let Some(target) =
                                            kamimusuhi_resident::status::route_target_label(route)
                                        {
                                            chip(
                                                ui,
                                                &target,
                                                if route["tier"].as_str() == Some("hai") {
                                                    AMBER
                                                } else {
                                                    GREEN
                                                },
                                            );
                                        }
                                        if let Some(cost) =
                                            kamimusuhi_resident::status::route_cost_label(route)
                                        {
                                            let color = match route["billing"].as_str() {
                                                Some("metered") => AMBER,
                                                _ => GREEN,
                                            };
                                            chip(ui, &cost, color);
                                        }
                                        if let Some(cached) =
                                            route["cached_tokens"].as_u64().filter(|c| *c > 0)
                                        {
                                            chip(ui, &format!("cache {cached}"), MUTED);
                                        }
                                    } else if let Some(tier) = tier {
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
                            if !tools.is_empty() {
                                ui.add_space(4.0);
                                egui::CollapsingHeader::new(
                                    RichText::new(format!("詳細 ({}件のツール呼出)", tools.len()))
                                        .small()
                                        .color(MUTED),
                                )
                                .id_salt(("tool-details", idx))
                                .show(ui, |ui| {
                                    for t in &tools {
                                        let name = t["name"]
                                            .as_str()
                                            .unwrap_or("?")
                                            .trim_start_matches("mcp__")
                                            .replace("__", "/");
                                        let ok = t["ok"].as_bool().unwrap_or(true);
                                        let ms = t["latency_ms"]
                                            .as_u64()
                                            .map(|m| format!(" {m}ms"))
                                            .unwrap_or_default();
                                        let ev = t["evidence_id"]
                                            .as_str()
                                            .map(|e| format!(" · {e}"))
                                            .unwrap_or_default();
                                        let task_info = t["task"]["id"]
                                            .as_str()
                                            .map(|tid| format!(" → task {tid}"))
                                            .unwrap_or_default();
                                        ui.label(
                                            RichText::new(format!(
                                                "{} {name}{ms}{task_info}{ev}",
                                                if ok { "✔" } else { "✘" }
                                            ))
                                            .small()
                                            .monospace()
                                            .color(if ok { TEXT } else { RED }),
                                        );
                                    }
                                });
                            }
                        }
                    });
                });
        },
    );
}

/// What a reply left running after the turn, as the chat shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Background {
    /// Tasks the individual delegated during the turn.
    Started(Vec<String>),
    /// A promise with no task behind it, waiting for the operator.
    Unstarted {
        id: String,
    },
    /// That promise, since started by the operator.
    Delegated(String),
    Cancelled,
}

fn task_label(task: &Value) -> String {
    let id = task["id"].as_str().unwrap_or("?");
    match task["executor"].as_str() {
        Some(executor) => format!("{id} · {executor}"),
        None => id.to_owned(),
    }
}

/// The reply's `background`, with a promise's state taken from the board
/// (which changes after the reply arrives).
fn background_of(meta: &Value, tasks: &[Value]) -> Option<Background> {
    let background = &meta["background"];
    match background["state"].as_str()? {
        "started" => Some(Background::Started(
            background["tasks"]
                .as_array()?
                .iter()
                .map(task_label)
                .collect(),
        )),
        "unstarted" => {
            let id = background["task_id"].as_str()?.to_owned();
            let task = tasks.iter().find(|t| t["id"].as_str() == Some(id.as_str()));
            Some(match task.map(|t| (t["status"].as_str(), t)) {
                Some((Some("done"), t)) if t["detail"]["delegated"].is_object() => {
                    Background::Delegated(task_label(&t["detail"]["delegated"]))
                }
                Some((Some("cancelled" | "done"), _)) => Background::Cancelled,
                _ => Background::Unstarted { id },
            })
        }
        _ => None,
    }
}

/// The strip under a reply. Returns (commitment id, start?) when clicked.
fn background_strip(ui: &mut egui::Ui, background: &Background) -> Option<(String, bool)> {
    let mut action = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.add_space(12.0);
        match background {
            Background::Started(tasks) => {
                for task in tasks {
                    chip(ui, &format!("▶ 裏で実行中 {task}"), GREEN);
                }
                ui.label(
                    RichText::new("進捗は「外部エージェント」で確認できます")
                        .small()
                        .color(MUTED),
                );
            }
            Background::Unstarted { id } => {
                chip(ui, "⚠ 引き受けたが未着手（タスク未作成）", AMBER);
                if ui
                    .small_button(RichText::new("▶ タスクとして開始").color(GREEN))
                    .on_hover_text("この依頼を外部エージェントに委譲し、裏で実行します")
                    .clicked()
                {
                    action = Some((id.clone(), true));
                }
                if ui.small_button("取り消し").clicked() {
                    action = Some((id.clone(), false));
                }
            }
            Background::Delegated(task) => chip(ui, &format!("▶ 委譲済み {task}"), GREEN),
            Background::Cancelled => chip(ui, "✕ 未着手のまま取り消し", MUTED),
        }
    });
    action
}

/// A board task only the operator can move on: a promise without a task.
fn needs_decision(task: &Value) -> bool {
    task["kind"] == "promise" && task["status"] == "awaiting_operator"
}

/// A delegated task that has stopped running.
fn delegation_ended(task: &Value) -> bool {
    matches!(
        task["status"].as_str(),
        Some("done" | "failed" | "cancelled")
    )
}

/// IDs in `ids` not seen before. The first call only records what is
/// there: those were known before this window opened.
fn newly_seen<'a>(
    seen: &mut Option<HashSet<String>>,
    ids: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let first = seen.is_none();
    let seen = seen.get_or_insert_with(HashSet::new);
    ids.filter(|id| seen.insert((*id).to_owned()))
        .filter(|_| !first)
        .map(str::to_owned)
        .collect()
}

fn clip_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn commitment_line(task: &Value) -> String {
    format!(
        "澪が引き受けた作業がタスクになっていません（{}）— {}",
        task["id"].as_str().unwrap_or("?"),
        clip_line(task["detail"]["message"].as_str().unwrap_or(""), 80)
    )
}

fn ended_line(task: &Value) -> String {
    let state = match task["status"].as_str() {
        Some("done") => "完了",
        Some("failed") => "失敗",
        _ => "取り消し",
    };
    let title = task["title"].as_str().unwrap_or("");
    format!(
        "▶ 委譲タスクが{state}しました（{}）— {}",
        task["id"].as_str().unwrap_or("?"),
        clip_line(title.strip_prefix("委譲: ").unwrap_or(title), 80)
    )
}

/// A card for a promise no task backs. Returns (id, start?) when clicked.
fn commitment_card(ui: &mut egui::Ui, task: &Value) -> Option<(String, bool)> {
    let id = task["id"].as_str().unwrap_or("").to_owned();
    let mut action = None;
    card(
        ui,
        "⚠ 判断待ち: 澪が引き受けたが、タスクは作成されていません",
        |ui| {
            for (label, key) in [("依頼", "message"), ("澪の返事", "response")] {
                if let Some(text) = task["detail"][key].as_str() {
                    ui.label(
                        RichText::new(format!("{label}: {}", clip_line(text, 200)))
                            .small()
                            .color(MUTED),
                    );
                }
            }
            ui.horizontal_wrapped(|ui| {
            if ui
                .button(RichText::new("▶ タスクとして開始").color(GREEN))
                .on_hover_text("外部エージェントに委譲して裏で実行します（依頼文に実行者名があればそれを使います）")
                .clicked()
            {
                action = Some((id.clone(), true));
            }
            if ui.button("取り消し").clicked() {
                action = Some((id.clone(), false));
            }
        });
        },
    );
    action
}

/// A system notification (macOS), so a decision is not missed while the
/// window is in the background.
fn os_notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let quote = |s: &str| {
            format!(
                "\"{}\"",
                clip_line(s, 200).replace('\\', "\\\\").replace('"', "\\\"")
            )
        };
        let script = format!(
            "display notification {} with title {}",
            quote(body),
            quote(title)
        );
        if let Ok(mut child) = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (title, body);
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
        "failed" | "cancelled" => "done",
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
        "commitment" => ("目", AMBER),
        "promise" => ("約", AMBER),
        "delegated" => ("委", GREEN),
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

    fn task_matches(&self, t: &Value) -> bool {
        if self.task_kind_filter != "all"
            && t["kind"].as_str() != Some(self.task_kind_filter.as_str())
        {
            return false;
        }
        let needle = self.task_filter.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let hay = format!(
            "{} {} {} {} {}",
            t["id"].as_str().unwrap_or(""),
            t["title"].as_str().unwrap_or(""),
            t["kind"].as_str().unwrap_or(""),
            t["node"].as_str().unwrap_or(""),
            t["owner"].as_str().unwrap_or(""),
        )
        .to_lowercase();
        hay.contains(&needle)
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
                ui.selectable_value(&mut self.map_mode, true, "マップ (⌘M)");
                ui.selectable_value(&mut self.map_mode, false, "ボード (⌘M)");
            });
        });
        ui.label(
            RichText::new("対話・ツール呼び出し（llm_master 転送含む）・承認・反映・定期ジョブが自動で載ります。ボードはカードを選ぶと前後がつながり、マップは全体の前提条件 → 後続と承認待ち（琥珀色）を表示します。")
                .small()
                .color(MUTED),
        );
        ui.separator();
        // Filter + New task composer.
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.task_filter)
                    .hint_text("🔍 タスクを検索…")
                    .desired_width(190.0),
            );
            if !self.task_filter.is_empty() && ui.small_button("✕").clicked() {
                self.task_filter.clear();
            }
            let kind_label = match self.task_kind_filter.as_str() {
                "all" => "すべての種類",
                "dialogue" => "対話 (dialogue)",
                "tool" => "ツール (tool)",
                "approval" => "承認 (approval)",
                "commit" => "反映 (commit)",
                "job" => "定期 (job)",
                "manual" => "手動 (manual)",
                "promise" => "約束 (promise)",
                "delegated" => "委譲 (delegated)",
                other => other,
            };
            egui::ComboBox::from_id_salt("task-kind-filter")
                .width(130.0)
                .selected_text(kind_label)
                .show_ui(ui, |ui| {
                    for (k, lbl) in [
                        ("all", "すべての種類"),
                        ("dialogue", "対話 (dialogue)"),
                        ("tool", "ツール (tool)"),
                        ("approval", "承認 (approval)"),
                        ("commit", "反映 (commit)"),
                        ("job", "定期 (job)"),
                        ("manual", "手動 (manual)"),
                        ("promise", "約束 (promise)"),
                        ("delegated", "委譲 (delegated)"),
                    ] {
                        ui.selectable_value(&mut self.task_kind_filter, k.to_owned(), lbl);
                    }
                });
            ui.separator();
            let title_resp = ui.add(
                egui::TextEdit::singleline(&mut self.new_task_title)
                    .hint_text("新しいタスク（澪にも見えます・Enterで追加）")
                    .desired_width(270.0),
            );
            if self.selected_task.is_some() {
                ui.checkbox(
                    &mut self.new_task_after_selected,
                    RichText::new("選択中の後に続ける").small().color(MUTED),
                );
            }
            let enter = title_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui
                .add(
                    egui::Button::new(RichText::new("＋ タスク追加").color(Color32::WHITE))
                        .fill(BLUE),
                )
                .clicked()
                || enter)
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
            340.0
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
                                let mut lane: Vec<&Value> = self
                                    .tasks
                                    .iter()
                                    .filter(|t| lane_of(t) == key && self.task_matches(t))
                                    .collect();
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
                            // Dependency links: faint for visible pairs, bright around the selection.
                            let painter = ui.painter();
                            let selected = self.selected_task.clone().unwrap_or_default();
                            for hot_pass in [false, true] {
                                for t in &self.tasks {
                                    let Some(to) = t["id"].as_str().and_then(|id| rects.get(id))
                                    else {
                                        continue;
                                    };
                                    for dep in t["depends_on"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .filter_map(Value::as_str)
                                    {
                                        let Some(from) = rects.get(dep) else { continue };
                                        let hot = !selected.is_empty()
                                            && (dep == selected
                                                || t["id"].as_str() == Some(selected.as_str()));
                                        if hot != hot_pass {
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
                                                1.0,
                                                Color32::from_rgba_unmultiplied(140, 160, 190, 55),
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
                                        draw_arrowhead(
                                            painter,
                                            b,
                                            c2,
                                            stroke.color,
                                            if hot { 6.5 } else { 4.5 },
                                        );
                                    }
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
        let status = t["status"].as_str().unwrap_or("");
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
        let failed = status == "failed";
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
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(4.0, CARD_H));
        painter.rect_filled(
            bar,
            egui::CornerRadius {
                nw: 10,
                sw: 10,
                ne: 0,
                se: 0,
            },
            status_color(status),
        );
        let (glyph, tile_color) = kind_tile(t["kind"].as_str().unwrap_or(""));
        let tile =
            egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 12.0), egui::vec2(36.0, 36.0));
        painter.rect_filled(tile, 8.0, tile_color.gamma_multiply(0.25));
        painter.text(
            tile.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(17.0),
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
            rect.min + egui::vec2(56.0, 10.0),
            egui::vec2(CARD_W - 90.0, 52.0),
        );
        ui.painter_at(clip).galley(clip.min, galley, TEXT);
        // Number badge.
        painter.text(
            rect.right_top() + egui::vec2(-10.0, 10.0),
            egui::Align2::RIGHT_TOP,
            number.to_string(),
            egui::FontId::proportional(11.0),
            MUTED,
        );
        // Footer: node, badges, owner, age.
        let node = t["node"].as_str().unwrap_or("?");
        let footer_y = rect.bottom() - 15.0;
        painter.text(
            egui::pos2(rect.left() + 12.0, footer_y),
            egui::Align2::LEFT_CENTER,
            format!("● {node}"),
            egui::FontId::proportional(11.0),
            node_color(node),
        );
        let notes_n = t["notes"].as_array().map_or(0, Vec::len);
        let deps_n = t["depends_on"].as_array().map_or(0, Vec::len);
        let mut badges = String::new();
        if deps_n > 0 {
            badges.push_str(&format!("⛓{deps_n} "));
        }
        if notes_n > 0 {
            badges.push_str(&format!("💬{notes_n}"));
        }
        if !badges.is_empty() {
            painter.text(
                egui::pos2(rect.left() + 82.0, footer_y),
                egui::Align2::LEFT_CENTER,
                badges.trim_end(),
                egui::FontId::proportional(10.5),
                MUTED,
            );
        }
        let owner = match t["owner"].as_str().unwrap_or("") {
            "operator" => "あなた",
            "mio" => "澪",
            _ => "system",
        };
        let when = t["duration_secs"].as_u64().map_or_else(
            || {
                t["updated"]
                    .as_str()
                    .map(|u| {
                        if u.is_ascii() && u.len() >= 16 {
                            u[11..16].to_owned()
                        } else {
                            u.chars().take(16).collect()
                        }
                    })
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
                egui::pos2(rect.right() - 10.0, rect.top() + 26.0),
                egui::Align2::RIGHT_TOP,
                "失敗",
                egui::FontId::proportional(10.5),
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
                    ui.add(
                        egui::Label::new(RichText::new(id).small().monospace().color(MUTED))
                            .selectable(true),
                    );
                    if ui
                        .small_button("📋")
                        .on_hover_text("タスク ID をコピー")
                        .clicked()
                    {
                        ui.ctx().copy_text(id.to_owned());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.small_button("閉じる (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add(
                    egui::Label::new(
                        RichText::new(t["title"].as_str().unwrap_or(""))
                            .strong()
                            .size(16.0)
                            .color(TEXT),
                    )
                    .wrap()
                    .selectable(true),
                );
                ui.add_space(6.0);
                let status_label = match t["status"].as_str().unwrap_or("") {
                    "in_progress" => "進行中",
                    "awaiting_operator" => "あなたの判断待ち",
                    "waiting" => "待機",
                    "on_hold" => "保留",
                    "done" => "完了",
                    "failed" => "失敗",
                    "cancelled" => "取消",
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
                        if let Some(subj) = t["detail"]["subject"].as_str() {
                            ui.label(RichText::new("subject").small().color(MUTED));
                            ui.label(RichText::new(subj).small().color(TEXT));
                            ui.end_row();
                        }
                        if let Some(exec) = t["detail"]["executor"].as_str() {
                            ui.label(RichText::new("実行者").small().color(MUTED));
                            ui.label(RichText::new(exec).small().color(TEXT));
                            ui.end_row();
                        }
                    });
                if let Some(msg) = t["detail"]["message"].as_str().filter(|s| !s.is_empty()) {
                    ui.add_space(4.0);
                    ui.label(RichText::new("依頼文").small().strong().color(MUTED));
                    ui.add(
                        egui::Label::new(RichText::new(clip_line(msg, 240)).small().color(TEXT))
                            .wrap()
                            .selectable(true),
                    );
                }
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
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "[{}] {}",
                                n["by"].as_str().unwrap_or(""),
                                n["text"].as_str().unwrap_or("")
                            ))
                            .small()
                            .color(TEXT),
                        )
                        .wrap()
                        .selectable(true),
                    );
                }
                if t["kind"] == "promise" && t["status"] == "awaiting_operator" {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("澪が引き受けたが、タスクは作成されていません。")
                            .small()
                            .color(AMBER),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button(RichText::new("▶ タスクとして開始").color(GREEN))
                            .clicked()
                        {
                            self.commitment_action(id, true);
                        }
                        if ui.button("取り消し").clicked() {
                            self.commitment_action(id, false);
                        }
                    });
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
                let note_edit = ui.add(
                    egui::TextEdit::multiline(&mut self.task_note)
                        .hint_text("メモを追加 (⌘/Ctrl+Enter で貼る)")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                let note_shortcut = note_edit.has_focus()
                    && ui.ctx().input(|i| {
                        i.key_pressed(egui::Key::Enter) && (i.modifiers.ctrl || i.modifiers.command)
                    });
                if (ui.button("メモを貼る").clicked() || note_shortcut)
                    && !self.task_note.trim().is_empty()
                {
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

fn draw_arrowhead(
    painter: &egui::Painter,
    tip: egui::Pos2,
    from: egui::Pos2,
    color: Color32,
    size: f32,
) {
    let dir = tip - from;
    let len = dir.length();
    if len < 0.5 {
        painter.circle_filled(tip, size * 0.45, color);
        return;
    }
    let u = dir / len;
    let perp = egui::vec2(-u.y, u.x);
    let back = tip - u * size * 1.4;
    let left = back + perp * size * 0.65;
    let right = back - perp * size * 0.65;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        color,
        Stroke::NONE,
    ));
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
        "cancelled" => MUTED,
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

fn map_cluster_bounds(
    tasks: &[&Value],
    rect_of: &std::collections::HashMap<&str, egui::Rect>,
) -> Vec<egui::Rect> {
    use std::collections::{HashMap, HashSet};
    let ids: HashSet<&str> = tasks.iter().filter_map(|t| t["id"].as_str()).collect();
    let mut parent: HashMap<&str, &str> = ids.iter().map(|i| (*i, *i)).collect();
    for t in tasks {
        let Some(id) = t["id"].as_str() else { continue };
        for dep in t["depends_on"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|d| ids.contains(d))
        {
            let (a, b) = (root_of(&mut parent, id), root_of(&mut parent, dep));
            if a != b {
                parent.insert(a, b);
            }
        }
    }
    let mut groups: HashMap<&str, (usize, egui::Rect)> = HashMap::new();
    for id in ids {
        let Some(rect) = rect_of.get(id) else {
            continue;
        };
        let root = root_of(&mut parent, id);
        groups
            .entry(root)
            .and_modify(|(cnt, box_r)| {
                *cnt += 1;
                *box_r = box_r.union(*rect);
            })
            .or_insert((1, *rect));
    }
    groups
        .into_values()
        .filter(|(cnt, _)| *cnt > 1)
        .map(|(_, r)| r.expand(10.0))
        .collect()
}

impl RemoteApp {
    fn task_map(&self, ui: &mut egui::Ui, clicked: &mut Option<String>) {
        let visible: Vec<&Value> = self
            .tasks
            .iter()
            .filter(|t| {
                (self.show_done
                    || !matches!(t["status"].as_str(), Some("done" | "failed" | "cancelled")))
                    && self.task_matches(t)
            })
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
        // Legend strip above map canvas.
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("表示 {} 件", visible.len()))
                    .small()
                    .color(MUTED),
            );
            for (lbl, col) in [
                ("進行中", BLUE),
                ("判断待ち", AMBER),
                ("待機", VIOLET),
                ("保留", MUTED),
                ("完了", GREEN),
                ("失敗", RED),
            ] {
                ui.label(RichText::new(format!("● {lbl}")).small().color(col));
            }
        });
        ui.add_space(4.0);
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
                // Cluster background frames for connected multi-task workflows.
                for cluster in map_cluster_bounds(&visible, &rect_of) {
                    painter.rect(
                        cluster,
                        12.0,
                        PANEL.gamma_multiply(0.85),
                        Stroke::new(1.0, BORDER.gamma_multiply(0.8)),
                        egui::StrokeKind::Inside,
                    );
                }
                // Edges first: prerequisite → successor.
                for hot_pass in [false, true] {
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
                            let hot = !selected.is_empty() && (dep == selected || *id == selected);
                            if hot != hot_pass {
                                continue;
                            }
                            let a = egui::pos2(from.right(), from.center().y);
                            let b = egui::pos2(to.left(), to.center().y);
                            let dx = ((b.x - a.x) * 0.5).max(30.0);
                            let c2 = egui::pos2(b.x - dx, b.y);
                            let color = if hot {
                                AMBER
                            } else {
                                Color32::from_rgba_unmultiplied(150, 170, 200, 120)
                            };
                            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                                [a, egui::pos2(a.x + dx, a.y), c2, b],
                                false,
                                Color32::TRANSPARENT,
                                Stroke::new(if hot { 2.4 } else { 1.4 }, color),
                            ));
                            draw_arrowhead(&painter, b, c2, color, if hot { 6.5 } else { 5.0 });
                        }
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
                        "cancelled" => "取消",
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
    fn only_decisions_that_appear_later_are_announced() {
        use serde_json::json;
        let mut seen = None;
        let first = [json!({"id": "c1", "kind": "promise", "status": "awaiting_operator"})];
        let ids = |tasks: &[Value]| -> Vec<String> {
            tasks
                .iter()
                .filter(|t| needs_decision(t))
                .filter_map(|t| t["id"].as_str().map(str::to_owned))
                .collect()
        };
        let first_ids = ids(&first);
        assert!(newly_seen(&mut seen, first_ids.iter().map(String::as_str)).is_empty());
        let later = [
            json!({"id": "c1", "kind": "promise", "status": "awaiting_operator"}),
            json!({"id": "c2", "kind": "promise", "status": "awaiting_operator"}),
            json!({"id": "c3", "kind": "promise", "status": "cancelled"}),
            json!({"id": "d1", "kind": "dialogue", "status": "awaiting_operator"}),
        ];
        let later_ids = ids(&later);
        assert_eq!(
            newly_seen(&mut seen, later_ids.iter().map(String::as_str)),
            vec!["c2".to_owned()]
        );
        assert!(newly_seen(&mut seen, later_ids.iter().map(String::as_str)).is_empty());
        assert!(delegation_ended(&json!({"status": "failed"})));
        assert!(!delegation_ended(&json!({"status": "running"})));
    }

    #[test]
    fn a_reply_shows_what_it_left_running() {
        use serde_json::json;
        let started = json!({"background": {"state": "started",
            "tasks": [{"id": "t1", "executor": "devin"}]}});
        assert_eq!(
            background_of(&started, &[]),
            Some(Background::Started(vec!["t1 · devin".to_owned()]))
        );
        let promise = json!({"background": {"state": "unstarted", "task_id": "c1"}});
        assert_eq!(
            background_of(&promise, &[]),
            Some(Background::Unstarted {
                id: "c1".to_owned()
            })
        );
        let board = [json!({"id": "c1", "status": "done",
            "detail": {"delegated": {"id": "t2", "executor": "opencode"}}})];
        assert_eq!(
            background_of(&promise, &board),
            Some(Background::Delegated("t2 · opencode".to_owned()))
        );
        let board = [json!({"id": "c1", "status": "cancelled"})];
        assert_eq!(background_of(&promise, &board), Some(Background::Cancelled));
        assert_eq!(background_of(&json!({"background": null}), &[]), None);
        assert_eq!(background_of(&json!({}), &[]), None);
    }

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
