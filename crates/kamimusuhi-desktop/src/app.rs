use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use eframe::egui::{self, Color32, FontDefinitions, FontFamily, RichText, Stroke, TextStyle};
use eframe::{App, CreationContext, Frame, NativeOptions};
use kamimusuhi_core::persona::ConversationRole;
use kamimusuhi_runtime::llm_jev::{
    ConversationCoreState, ConversationTrace, GeneratedLanguageCandidate,
};

use crate::worker::{ConnectionSummary, ReadyState, WorkerCommand, WorkerEvent};

const BG: Color32 = Color32::from_rgb(8, 15, 24);
const PANEL: Color32 = Color32::from_rgb(13, 24, 36);
const PANEL_RAISED: Color32 = Color32::from_rgb(19, 32, 47);
const BORDER: Color32 = Color32::from_rgb(37, 57, 78);
const TEXT: Color32 = Color32::from_rgb(224, 233, 244);
const MUTED: Color32 = Color32::from_rgb(139, 160, 185);
const BLUE: Color32 = Color32::from_rgb(49, 129, 235);
const GREEN: Color32 = Color32::from_rgb(47, 206, 132);
const AMBER: Color32 = Color32::from_rgb(243, 181, 62);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Status,
    Trace,
    Connections,
}

#[derive(Debug, Clone)]
struct UiMessage {
    role: ConversationRole,
    text: String,
}

struct GenerationReport {
    candidates: Vec<GeneratedLanguageCandidate>,
    elapsed_ms: u64,
}

pub fn run(commands: Sender<WorkerCommand>, events: Receiver<WorkerEvent>) -> Result<(), String> {
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title("Kamimusuhi / かみむすび"),
        ..Default::default()
    };
    eframe::run_native(
        "Kamimusuhi",
        options,
        Box::new(move |cc| Ok(Box::new(NativeApp::new(cc, commands, events)))),
    )
    .map_err(|error| error.to_string())
}

struct NativeApp {
    commands: Sender<WorkerCommand>,
    events: Receiver<WorkerEvent>,
    messages: Vec<UiMessage>,
    input: String,
    view: View,
    busy: bool,
    next_request_id: u64,
    last_error: Option<String>,
    session_id: String,
    individual_id: String,
    subject: String,
    connection: Option<ConnectionSummary>,
    trace: Option<ConversationTrace>,
    generation_report: Option<GenerationReport>,
    core_state: Option<ConversationCoreState>,
    turn_count: u64,
    history_messages: usize,
    last_elapsed_ms: Option<u128>,
    new_provider_id: String,
    new_provider_base_url: String,
    new_provider_model: String,
    new_provider_auth_env: String,
    closed: bool,
}

impl NativeApp {
    fn new(
        cc: &CreationContext<'_>,
        commands: Sender<WorkerCommand>,
        events: Receiver<WorkerEvent>,
    ) -> Self {
        configure_theme(&cc.egui_ctx);
        Self::with_channels(commands, events)
    }

    fn with_channels(commands: Sender<WorkerCommand>, events: Receiver<WorkerEvent>) -> Self {
        Self {
            commands,
            events,
            messages: Vec::new(),
            input: String::new(),
            view: View::Chat,
            busy: false,
            next_request_id: 1,
            last_error: None,
            session_id: "起動中…".to_owned(),
            individual_id: "—".to_owned(),
            subject: "local-user".to_owned(),
            connection: None,
            trace: None,
            generation_report: None,
            core_state: None,
            turn_count: 0,
            history_messages: 0,
            last_elapsed_ms: None,
            new_provider_id: "assistant-2".to_owned(),
            new_provider_base_url: "http://127.0.0.1:11434/v1".to_owned(),
            new_provider_model: String::new(),
            new_provider_auth_env: String::new(),
            closed: false,
        }
    }

    fn poll_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                WorkerEvent::Ready(ready) => self.apply_ready(ready),
                WorkerEvent::GenerationReport {
                    candidates,
                    elapsed_ms,
                } => {
                    self.generation_report = Some(GenerationReport {
                        candidates,
                        elapsed_ms,
                    });
                }
                WorkerEvent::Reply {
                    response,
                    trace,
                    elapsed_ms,
                    history_messages,
                    core_state,
                } => {
                    self.busy = false;
                    self.last_error = None;
                    self.messages.push(UiMessage {
                        role: ConversationRole::Assistant,
                        text: response,
                    });
                    self.trace = trace.map(|trace| *trace);
                    self.core_state = core_state;
                    self.turn_count = self.turn_count.saturating_add(1);
                    self.history_messages = history_messages;
                    self.last_elapsed_ms = Some(elapsed_ms);
                }
                WorkerEvent::Error { message } => {
                    self.busy = false;
                    self.last_error = Some(message);
                }
                WorkerEvent::ProvidersUpdated(providers) => {
                    if let Some(connection) = &mut self.connection {
                        connection.language_providers = providers;
                        self.last_error = None;
                        self.new_provider_id.clear();
                        self.new_provider_base_url.clear();
                        self.new_provider_model.clear();
                        self.new_provider_auth_env.clear();
                    }
                }
                WorkerEvent::Stopped => self.closed = true,
            }
        }
        if self.busy || self.connection.is_none() {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
    }

    fn apply_ready(&mut self, ready: ReadyState) {
        self.individual_id = ready.individual_id;
        self.session_id = ready.session_id;
        self.subject = ready.subject;
        self.connection = Some(ready.connection);
        self.history_messages = ready.history.len();
        self.messages = ready
            .history
            .into_iter()
            .map(|message| UiMessage {
                role: message.role,
                text: message.text,
            })
            .collect();
    }

    fn add_provider_from_form(&mut self) {
        if self.busy || self.connection.is_none() {
            return;
        }
        let id = self.new_provider_id.trim().to_owned();
        let base_url = self.new_provider_base_url.trim().to_owned();
        let model = self.new_provider_model.trim().to_owned();
        let auth_env = self.new_provider_auth_env.trim().to_owned();
        if id.is_empty() || base_url.is_empty() || model.is_empty() {
            self.last_error = Some("API ID・Base URL・モデルを入力してください。".to_owned());
            return;
        }
        if self
            .commands
            .send(WorkerCommand::AddLanguageProvider {
                id,
                base_url,
                model,
                auth_env: (!auth_env.is_empty()).then_some(auth_env),
            })
            .is_err()
        {
            self.last_error = Some("対話ワーカーへ接続できません。".to_owned());
        }
    }

    fn remove_provider(&mut self, id: &str) {
        if self.busy {
            return;
        }
        if self
            .commands
            .send(WorkerCommand::RemoveLanguageProvider { id: id.to_owned() })
            .is_err()
        {
            self.last_error = Some("対話ワーカーへ接続できません。".to_owned());
        }
    }

    fn send_current(&mut self) {
        if self.busy || self.connection.is_none() {
            return;
        }
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.trace = None;
        self.generation_report = None;
        self.last_elapsed_ms = None;
        if text.len() > kamimusuhi_runtime::dialogue::MAX_INPUT_BYTES {
            self.last_error = Some("入力が8,192 UTF-8 bytesを超えています。".to_owned());
            return;
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.messages.push(UiMessage {
            role: ConversationRole::User,
            text: text.clone(),
        });
        self.input.clear();
        self.last_error = None;
        self.busy = true;
        if self
            .commands
            .send(WorkerCommand::Send { request_id, text })
            .is_err()
        {
            self.busy = false;
            self.last_error = Some("対話ワーカーへ接続できません。".to_owned());
        }
    }

    fn top_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("◈ かみむすび")
                    .strong()
                    .size(19.0)
                    .color(TEXT),
            );
            ui.label(RichText::new("会話側 K-CORE").small().color(MUTED));
            ui.add_space(16.0);
            ui.label(
                RichText::new(format!("subject: {}", self.subject))
                    .small()
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, color) = if self.busy {
                    ("処理中", AMBER)
                } else if self.connection.is_some() {
                    ("● オンライン", GREEN)
                } else {
                    ("○ 起動待ち", MUTED)
                };
                ui.label(RichText::new(label).small().color(color));
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("KAMIMUSUHI").small().strong().color(MUTED));
        ui.label(RichText::new("対話と状態の観測面").small().color(MUTED));
        ui.add_space(20.0);
        self.nav_button(ui, "▣  チャット", View::Chat);
        self.nav_button(ui, "◌  状態", View::Status);
        self.nav_button(ui, "⌁  内部トレース", View::Trace);
        self.nav_button(ui, "⚙  接続設定", View::Connections);
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(10.0);
        ui.label(RichText::new("会話").small().color(MUTED));
        ui.label(RichText::new("現在のセッション").color(TEXT));
        ui.label(
            RichText::new(format!("{} turn", self.turn_count))
                .small()
                .color(MUTED),
        );
        ui.add_space(16.0);
        ui.label(RichText::new("準備中").small().color(MUTED));
        for label in ["メモリ", "エージェント", "音声入出力"] {
            ui.add_enabled(
                false,
                egui::Button::new(RichText::new(format!("○  {label}"))),
            );
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.separator();
            ui.add_space(8.0);
            ui.label(RichText::new("Runtime / SQLite").small().color(MUTED));
            ui.label(
                RichText::new(short_id(&self.individual_id))
                    .small()
                    .color(TEXT),
            );
        });
    }

    fn nav_button(&mut self, ui: &mut egui::Ui, label: &str, view: View) {
        let selected = self.view == view;
        let response = ui.add_sized(
            [ui.available_width(), 34.0],
            egui::Button::new(RichText::new(label).color(if selected { TEXT } else { MUTED }))
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
        if response.clicked() {
            self.view = view;
        }
    }

    fn chat_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("対話").heading().color(TEXT));
            ui.label(
                RichText::new(format!("session {}", short_id(&self.session_id)))
                    .small()
                    .color(MUTED),
            );
        });
        ui.separator();
        // A bare ScrollArea can consume all remaining height, which pushes the
        // composer below the central panel and makes it appear to be missing.
        // Reserve the composer area first so the input is always visible.
        let composer_height = 118.0 + if self.last_error.is_some() { 42.0 } else { 0.0 };
        let history_height = (ui.available_height() - composer_height).max(120.0);
        let history_width = ui.available_width();
        ui.allocate_ui(egui::vec2(history_width, history_height), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("chat-scroll")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.messages.is_empty() {
                        ui.add_space(90.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("まだ会話はありません").size(20.0).color(TEXT));
                            ui.label(
                                RichText::new("下の入力欄から日本語で話しかけてください")
                                    .color(MUTED),
                            );
                        });
                    }
                    for message in &self.messages {
                        message_bubble(ui, message);
                        ui.add_space(10.0);
                    }
                    if self.busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new(
                                    "発話・記憶・不足情報の判定 → 全対象器官の並行生成 → 候補別の根拠確認と選択",
                                )
                                    .small()
                                    .color(MUTED),
                            );
                        });
                    }
                });
        });

        if let Some(error) = &self.last_error {
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

        let frame = egui::Frame::new()
            .fill(PANEL_RAISED)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(8.0);
        frame.show(ui, |ui| {
            let available = ui.available_width();
            let text_edit = egui::TextEdit::multiline(&mut self.input)
                .hint_text("メッセージを入力…  (Ctrl / Cmd + Enter で送信)")
                .desired_rows(3)
                .desired_width((available - 92.0).max(240.0));
            let response = ui.add_enabled(!self.busy, text_edit);
            let shortcut = response.has_focus()
                && ctx.input(|input| {
                    input.key_pressed(egui::Key::Enter)
                        && (input.modifiers.ctrl || input.modifiers.command)
                });
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Enter: 改行  /  Ctrl+Enter: 送信")
                        .small()
                        .color(MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.busy,
                            egui::Button::new(
                                RichText::new("送信  ➤").strong().color(Color32::WHITE),
                            )
                            .fill(BLUE),
                        )
                        .clicked()
                        || shortcut
                    {
                        self.send_current();
                    }
                });
            });
        });
    }

    fn status_view(&self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("使用量・状態").color(TEXT));
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            stat_card(
                ui,
                "TURN",
                &self.turn_count.to_string(),
                "会話側K-CORE",
                BLUE,
            );
            stat_card(
                ui,
                "履歴",
                &self.history_messages.to_string(),
                "subject-scoped",
                GREEN,
            );
            stat_card(
                ui,
                "遅延",
                &self
                    .last_elapsed_ms
                    .map_or_else(|| "—".to_owned(), |value| format!("{value} ms")),
                "直近の成功ターン",
                AMBER,
            );
        });
        ui.add_space(16.0);
        card(ui, "会話用 K-CORE state", |ui| {
            if let Some(state) = &self.core_state {
                key_value(ui, "turn", &state.turn.to_string());
                key_value(ui, "speech act", &state.speech_act);
                key_value(ui, "goal", state.active_goal.as_deref().unwrap_or("—"));
                key_value(ui, "attention", &state.attention.join(" / "));
                key_value(ui, "uncertainty", &format!("{:.2}", state.uncertainty));
                key_value(ui, "arousal", &format!("{:.2}", state.arousal));
                key_value(
                    ui,
                    "last action",
                    state.last_action.as_deref().unwrap_or("—"),
                );
            } else {
                ui.label(RichText::new("最初の対話を待っています。").color(MUTED));
            }
        });
    }

    fn trace_view(&self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("内部トレース").color(TEXT));
        ui.label(
            RichText::new(
                "モデルの内部思考ではなく、公開された境界イベントとtyped choiceだけを表示します。",
            )
            .small()
            .color(MUTED),
        );
        ui.separator();
        if self.busy {
            ui.label(
                RichText::new("処理中。生成レポートはターン終了時にまとめて表示します。")
                    .small()
                    .color(MUTED),
            );
        }
        if let Some(error) = &self.last_error {
            ui.label(RichText::new(error).color(AMBER));
        }
        if let Some(trace) = &self.trace {
            gate_card(
                ui,
                "invocation_gate",
                trace.invocation_gate.decision.as_str(),
                trace.invocation_gate.confidence,
                trace.invocation_gate.fallback,
                trace.invocation_gate.latency_ms,
            );
            ui.add_space(8.0);
        }
        if let Some((candidates, elapsed_ms)) = self.generation_details() {
            generated_candidates_card(ui, candidates, elapsed_ms, self.trace.as_ref());
            ui.add_space(8.0);
        }
        if let Some(trace) = &self.trace {
            card(ui, "response_candidate / 生成後の応答選択", |ui| {
                key_value(ui, "selected id", &trace.language_provider_id);
                key_value(ui, "provider", &trace.language_provider);
                key_value(ui, "model", &trace.language_model);
                key_value(
                    ui,
                    "selected generation",
                    &format!("{} ms", trace.language_latency_ms),
                );
                key_value(ui, "retry count", &trace.retry_count.to_string());
                if let Some(selection) = &trace.provider_selection {
                    key_value(ui, "decision provider", &selection.provider);
                    key_value(ui, "response_candidate", &selection.provider_id);
                    key_value(
                        ui,
                        "assessment latency (final batch)",
                        &format!("{} ms", selection.latency_ms),
                    );
                    key_value(
                        ui,
                        "selection confidence",
                        &format!("{:.2}", selection.confidence),
                    );
                }
                if let Some(telemetry) = trace.provider_telemetry.get(&trace.language_provider_id) {
                    key_value(
                        ui,
                        "session EWMA",
                        &telemetry
                            .ewma_latency_ms
                            .map_or_else(|| "unknown".to_owned(), |value| format!("{value} ms")),
                    );
                    key_value(
                        ui,
                        "session success rate",
                        &telemetry.success_rate.map_or_else(
                            || "unknown".to_owned(),
                            |value| format!("{:.0}%", value * 100.0),
                        ),
                    );
                }
            });
            ui.add_space(8.0);
            gate_card(
                ui,
                "response_gate",
                trace.response_gate.decision.as_str(),
                trace.response_gate.confidence,
                trace.response_gate.fallback,
                trace.response_gate.latency_ms,
            );
            ui.label(
                RichText::new("Jevの選択とresponse gateは同じ一括判定です。遅延は合算しません。confidenceは元のJev判定の分布由来の値で、実効判定の正答率ではありません。")
                    .small()
                    .color(MUTED),
            );
            if let Some(assessment) = trace.assessments.last() {
                card(ui, "候補の根拠確認", |ui| {
                    key_value(ui, "根拠", &format!("{:?}", assessment.grounding));
                    key_value(
                        ui,
                        "自己・他者の帰属",
                        &format!("{:?}", assessment.attribution),
                    );
                    key_value(ui, "依頼への適合", &format!("{:?}", assessment.task_fit));
                    key_value(ui, "修正理由", assessment.repair_reason.as_str());
                    key_value(ui, "Jevの元の判定", assessment.raw_gate.as_str());
                });
            }
            ui.add_space(12.0);
            egui::CollapsingHeader::new("構造化trace JSON").show(ui, |ui| {
                let mut json =
                    serde_json::to_string_pretty(trace).unwrap_or_else(|_| "{}".to_owned());
                ui.add(egui::TextEdit::multiline(&mut json).font(egui::TextStyle::Monospace));
            });
        } else if self.generation_report.is_some() {
            ui.label(
                RichText::new("成功トレースなし。生成レポートのみ表示し、採用マークは付けません。")
                    .small()
                    .color(MUTED),
            );
        } else if !self.busy {
            ui.label(RichText::new("このターンのトレースはありません。").color(MUTED));
        }
    }

    fn generation_details(&self) -> Option<(&[GeneratedLanguageCandidate], u64)> {
        self.trace
            .as_ref()
            .map(|trace| {
                (
                    trace.generated_candidates.as_slice(),
                    trace.generation_latency_ms,
                )
            })
            .or_else(|| {
                self.generation_report
                    .as_ref()
                    .map(|report| (report.candidates.as_slice(), report.elapsed_ms))
            })
    }

    fn connections_view(&self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("接続設定").color(TEXT));
        ui.label(
            RichText::new("認証値は表示・保存せず、プロセス環境からのみ読み取ります。")
                .small()
                .color(MUTED),
        );
        ui.separator();
        if let Some(connection) = &self.connection {
            provider_card(
                ui,
                "Jev / TypeSafe System One",
                &connection.jev_model,
                &connection.jev_base_url,
                connection.jev_configured,
                "TYPESAFE_API_KEY",
            );
            ui.add_space(10.0);
            provider_card(
                ui,
                "primary 言語器官",
                &connection.llm_model,
                &connection.llm_base_url,
                !matches!(connection.llm_provider.as_str(), "mock" | "in-process"),
                &connection.auth_variables.join(" / "),
            );
            ui.add_space(14.0);
            card(ui, "経路", |ui| {
                ui.label(RichText::new("DialogueSession").color(TEXT));
                ui.label(RichText::new("↓ 会話用 K-CORE state").color(MUTED));
                ui.label(RichText::new("↓ Jev : 発話・記憶・不足情報を一括判定").color(BLUE));
                ui.label(RichText::new("↓ 全対象器官 /chat/completions を並行生成").color(GREEN));
                ui.label(RichText::new("↓ 全生成の完了・timeoutを待つ").color(MUTED));
                ui.label(
                    RichText::new("↓ Jev : 候補選択・根拠・帰属・適合・gateを一括判定").color(BLUE),
                );
            });
            ui.add_space(10.0);
            card(ui, "生成・選択のルール", |ui| {
                for note in [
                    "primaryを含め、有効・privacy許可・必要な認証が設定済みの全器官で生成します。認証不要の器官も対象です。",
                    "全器官の完了またはtimeoutを待つため、初回生成の待ち時間は最も遅い器官に左右されます。",
                    "生成後に応答の品質を優先して選択し、レイテンシ・成功率は補助情報として使います。空の応答と16 KiB超の応答を除外し、有効な応答の全文を評価します。",
                    "RETRYは修正理由を添えて選択器官だけを1回再生成し、更新した候補群全体から再選択・再判定します。",
                    "設定済みJevの失敗を自動ACCEPTに置き換えません。Jevのキーが未設定ならrule-basedで動作し、Mockにも対応します。",
                    "遅延は各呼出しの計測値、統計はこのセッション内だけの観測値です。実運用のスループットやtokens/sを示す値ではありません。",
                ] {
                    ui.label(RichText::new(note).small().color(MUTED));
                    ui.add_space(4.0);
                }
            });
            if !connection.language_providers.is_empty() {
                ui.add_space(10.0);
                card(ui, "登録済み追加API", |ui| {
                    for provider in &connection.language_providers {
                        key_value(
                            ui,
                            &provider.id,
                            &format!(
                                "{} / {} / {}",
                                provider.provider, provider.model, provider.base_url
                            ),
                        );
                    }
                });
            }
        } else {
            ui.label(RichText::new("ワーカーを起動しています…").color(MUTED));
        }
    }
}

impl App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.poll_events(ctx);
        egui::TopBottomPanel::top("topbar")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(12.0))
            .show(ctx, |ui| self.top_bar(ui));
        egui::SidePanel::left("navigation")
            .frame(egui::Frame::new().fill(BG).inner_margin(12.0))
            .resizable(false)
            .default_width(196.0)
            .show(ctx, |ui| self.nav(ui));
        egui::SidePanel::right("inspector")
            .frame(egui::Frame::new().fill(BG).inner_margin(10.0))
            .default_width(320.0)
            .min_width(270.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector-scroll")
                    .show(ui, |ui| self.inspector(ui));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(16.0))
            .show(ctx, |ui| match self.view {
                View::Chat => self.chat_view(ui, ctx),
                View::Status => self.status_view(ui),
                View::Trace => {
                    egui::ScrollArea::vertical()
                        .id_salt("trace-scroll")
                        .show(ui, |ui| self.trace_view(ui));
                }
                View::Connections => {
                    egui::ScrollArea::vertical()
                        .id_salt("connections-scroll")
                        .show(ui, |ui| self.connections_view(ui));
                }
            });
    }
}

impl Drop for NativeApp {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.commands.send(WorkerCommand::Shutdown);
        }
    }
}

fn configure_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.faint_bg_color = BG;
    visuals.extreme_bg_color = BG;
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.fg_stroke.color = TEXT;
    visuals.widgets.inactive.bg_fill = PANEL_RAISED;
    visuals.widgets.inactive.fg_stroke.color = MUTED;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(26, 51, 79);
    visuals.widgets.hovered.fg_stroke.color = TEXT;
    visuals.widgets.active.bg_fill = Color32::from_rgb(29, 79, 138);
    visuals.widgets.active.fg_stroke.color = TEXT;
    visuals.selection.bg_fill = BLUE;
    visuals.selection.stroke.color = Color32::WHITE;
    ctx.set_visuals(visuals);

    let mut fonts = FontDefinitions::default();
    for (name, path) in [
        (
            "system-japanese",
            "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        ),
        (
            "system-japanese",
            "/System/Library/Fonts/ヒラギノ角ゴシック W6.ttc",
        ),
        (
            "system-japanese",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        ),
        (
            "system-japanese",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.insert(0, name.to_owned());
                }
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
    ctx.style_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Body, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Heading, egui::FontId::proportional(20.0));
    });
}

impl NativeApp {
    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("状態 / SESSION")
                .small()
                .strong()
                .color(MUTED),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            stat_card(ui, "TURN", &self.turn_count.to_string(), "会話", BLUE);
            stat_card(
                ui,
                "LATENCY",
                &self
                    .last_elapsed_ms
                    .map_or_else(|| "—".to_owned(), |ms| format!("{ms}ms")),
                "成功ターン",
                AMBER,
            );
        });
        ui.add_space(8.0);
        card(ui, "会話側 K-CORE", |ui| {
            if let Some(state) = &self.core_state {
                key_value(
                    ui,
                    "state",
                    &format!("turn {} / {}", state.turn, state.speech_act),
                );
                key_value(ui, "goal", state.active_goal.as_deref().unwrap_or("—"));
                key_value(ui, "uncertainty", &format!("{:.2}", state.uncertainty));
            } else {
                ui.label(RichText::new("未観測").color(MUTED));
            }
        });
        ui.add_space(8.0);
        card(ui, "最新トレース", |ui| {
            if let Some(trace) = &self.trace {
                key_value(ui, "invocation", trace.invocation_gate.decision.as_str());
                key_value(ui, "response_candidate", &trace.language_provider_id);
                key_value(ui, "selected model", &trace.language_model);
                key_value(ui, "response", trace.response_gate.decision.as_str());
                key_value(ui, "retry", &trace.retry_count.to_string());
            } else if self.generation_report.is_some() {
                ui.label(RichText::new("成功トレースなし / 生成レポートあり").color(AMBER));
            } else {
                ui.label(RichText::new("ターン終了後に表示").color(MUTED));
            }
            if let Some((candidates, elapsed_ms)) = self.generation_details() {
                key_value(
                    ui,
                    "生成試行数 (primary・retry含む)",
                    &candidates.len().to_string(),
                );
                key_value(ui, "初回並行生成", &format!("{elapsed_ms} ms"));
                if ui.link("全候補・エラーを内部トレースで見る").clicked() {
                    self.view = View::Trace;
                }
            }
        });
        ui.add_space(8.0);
        card(ui, "Extra LLM API", |ui| {
            ui.label(
                RichText::new("OpenAI-compatible /chat/completions を追加登録")
                    .small()
                    .color(MUTED),
            );
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.label(RichText::new("API ID").small().color(MUTED));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_provider_id).hint_text("assistant-2"),
                );
                ui.label(RichText::new("Base URL").small().color(MUTED));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_provider_base_url)
                        .hint_text("http://127.0.0.1:11434/v1"),
                );
                ui.label(RichText::new("モデル").small().color(MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.new_provider_model).hint_text("model"));
                ui.label(
                    RichText::new("認証環境変数名（値は入力しない）")
                        .small()
                        .color(MUTED),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_provider_auth_env)
                        .hint_text("LLM_API_KEY"),
                );
                if ui
                    .add_enabled(
                        self.connection.is_some(),
                        egui::Button::new(RichText::new("＋ APIを登録").color(Color32::WHITE))
                            .fill(BLUE),
                    )
                    .clicked()
                {
                    self.add_provider_from_form();
                }
            });
            ui.add_space(6.0);
            let providers = self
                .connection
                .as_ref()
                .map(|connection| connection.language_providers.clone())
                .unwrap_or_default();
            if providers.is_empty() {
                ui.label(
                    RichText::new("追加登録なし。primaryを使用します。")
                        .small()
                        .color(MUTED),
                );
            } else {
                let mut remove_id = None;
                for provider in &providers {
                    ui.horizontal(|ui| {
                        let status = if provider.active {
                            "●"
                        } else {
                            "○ inactive"
                        };
                        ui.label(RichText::new(status).small().color(if provider.active {
                            GREEN
                        } else {
                            AMBER
                        }));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&provider.id).strong().color(TEXT));
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · {} · {}",
                                    provider.provider,
                                    provider.model,
                                    provider.base_url,
                                    provider_telemetry_label(&provider.telemetry),
                                ))
                                .small()
                                .color(MUTED),
                            );
                        });
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("削除"))
                            .clicked()
                        {
                            remove_id = Some(provider.id.clone());
                        }
                    });
                }
                if let Some(id) = remove_id {
                    self.remove_provider(&id);
                }
                ui.label(
                    RichText::new(
                        "許可された全APIを並行生成し、response_candidateで応答の品質を優先して選択します。セッション内の遅延・成功率は補助情報です。",
                    )
                    .small()
                    .color(MUTED),
                );
            }
        });
    }
}

fn message_bubble(ui: &mut egui::Ui, message: &UiMessage) {
    let user = message.role == ConversationRole::User;
    let fill = if user {
        Color32::from_rgb(25, 75, 145)
    } else {
        PANEL_RAISED
    };
    let stroke = if user {
        Color32::from_rgb(45, 121, 222)
    } else {
        BORDER
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
                .corner_radius(egui::CornerRadius::same(9))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(720.0));
                    ui.label(
                        RichText::new(if user { "あなた" } else { "かみむすび" })
                            .small()
                            .strong()
                            .color(if user {
                                Color32::from_rgb(183, 218, 255)
                            } else {
                                Color32::from_rgb(133, 219, 184)
                            }),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(&message.text).color(TEXT));
                });
        },
    );
}

fn stat_card(ui: &mut egui::Ui, label: &str, value: &str, note: &str, color: Color32) {
    egui::Frame::new()
        .fill(PANEL_RAISED)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.set_min_width(92.0);
            ui.label(RichText::new(label).small().color(MUTED));
            ui.label(RichText::new(value).strong().size(20.0).color(color));
            ui.label(RichText::new(note).small().color(MUTED));
        });
}

fn card(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong().color(TEXT));
            ui.add_space(6.0);
            content(ui);
        });
}

fn selected_candidate_index(
    candidates: &[GeneratedLanguageCandidate],
    trace: Option<&ConversationTrace>,
) -> Option<usize> {
    let trace = trace?;
    // A retry can reuse the ID, or even the same response text. Mark only the
    // latest successful attempt bound to the final accepted ID and digest.
    candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.id == trace.language_provider_id
                && candidate.response_digest.as_deref() == Some(trace.candidate_digest.as_str())
                && candidate.error_code.is_none()
        })
        .max_by_key(|(_, candidate)| candidate.attempt)
        .map(|(index, _)| index)
}

fn generated_candidates_card(
    ui: &mut egui::Ui,
    candidates: &[GeneratedLanguageCandidate],
    elapsed_ms: u64,
    trace: Option<&ConversationTrace>,
) {
    let selected_index = selected_candidate_index(candidates, trace);
    card(ui, "全生成候補 / primary・retryを含む", |ui| {
        key_value(ui, "初回並行生成 (wall time)", &format!("{elapsed_ms} ms"));
        ui.label(
            RichText::new("全器官の完了・timeoutまでの待ち時間です。各器官の時間の合計ではなく、retryは別計測です。")
                .small()
                .color(MUTED),
        );
        if candidates.is_empty() {
            ui.label(RichText::new("生成試行なし（生成前の中断を含む）。").color(MUTED));
        }
        for (index, candidate) in candidates.iter().enumerate() {
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&candidate.id).strong().color(TEXT));
                ui.label(
                    RichText::new(format!(
                        "attempt={} ({})",
                        candidate.attempt,
                        if candidate.attempt == 0 {
                            "初回"
                        } else {
                            "retry"
                        },
                    ))
                    .small()
                    .color(MUTED),
                );
                if selected_index == Some(index) {
                    ui.label(RichText::new("✓ 採用").strong().color(GREEN));
                }
            });
            key_value(
                ui,
                "provider / model",
                &format!("{} / {}", candidate.provider, candidate.model),
            );
            key_value(
                ui,
                "generation latency",
                &format!("{} ms", candidate.latency_ms),
            );
            key_value(
                ui,
                "response bytes (UTF-8)",
                &candidate
                    .response_bytes
                    .map_or_else(|| "—".to_owned(), |bytes| bytes.to_string()),
            );
            key_value(
                ui,
                "error",
                candidate.error_code.as_deref().unwrap_or("なし"),
            );
            key_value(
                ui,
                "session telemetry",
                &provider_telemetry_label(&candidate.telemetry),
            );
        }
    });
}

fn gate_card(
    ui: &mut egui::Ui,
    title: &str,
    decision: &str,
    confidence: f32,
    fallback: bool,
    latency_ms: u64,
) {
    card(ui, title, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(decision)
                    .strong()
                    .size(17.0)
                    .color(if fallback { AMBER } else { GREEN }),
            );
            ui.label(
                RichText::new(format!("confidence {:.2}", confidence))
                    .small()
                    .color(MUTED),
            );
            if fallback {
                ui.label(RichText::new("fallback").small().color(AMBER));
            }
        });
        key_value(ui, "call latency", &format!("{latency_ms} ms"));
    });
}

fn provider_card(
    ui: &mut egui::Ui,
    title: &str,
    model: &str,
    base_url: &str,
    configured: bool,
    auth: &str,
) {
    card(ui, title, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if configured { "●" } else { "○" }).color(if configured {
                    GREEN
                } else {
                    MUTED
                }),
            );
            ui.label(
                RichText::new(if configured {
                    "接続設定あり"
                } else {
                    "外部設定なし / ローカル動作"
                })
                .color(if configured { GREEN } else { MUTED }),
            );
        });
        key_value(ui, "model", model);
        key_value(ui, "base URL", base_url);
        key_value(
            ui,
            "credential",
            if auth.is_empty() {
                "なし"
            } else {
                "環境変数のみ"
            },
        );
    });
}

fn provider_telemetry_label(
    telemetry: &kamimusuhi_runtime::llm_jev::LanguageProviderTelemetrySnapshot,
) -> String {
    let latency = telemetry
        .ewma_latency_ms
        .map_or_else(|| "latency=?".to_owned(), |value| format!("ewma={value}ms"));
    let success = telemetry.success_rate.map_or_else(
        || "success=?".to_owned(),
        |value| format!("success={:.0}%", value * 100.0),
    );
    format!("session calls={}, {latency}, {success}", telemetry.calls)
}

fn key_value(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{key}: ")).small().color(MUTED));
        ui.label(RichText::new(value).small().color(TEXT));
    });
}

fn short_id(value: &str) -> String {
    if value.len() <= 18 {
        value.to_owned()
    } else {
        format!("{}…", &value[..18])
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use kamimusuhi_runtime::llm_jev::{Decision, DecisionResult, LanguageProviderTelemetry};

    use super::*;

    fn candidate(id: &str, attempt: u8, digest: &str) -> GeneratedLanguageCandidate {
        GeneratedLanguageCandidate {
            id: id.to_owned(),
            attempt,
            provider: "mock".to_owned(),
            model: "fixture".to_owned(),
            latency_ms: 10,
            response_bytes: Some(12),
            response_digest: Some(digest.to_owned()),
            error_code: None,
            telemetry: LanguageProviderTelemetry::default().snapshot(),
        }
    }

    fn trace(
        candidates: Vec<GeneratedLanguageCandidate>,
        id: &str,
        digest: &str,
    ) -> ConversationTrace {
        let gate = |decision: Decision| DecisionResult {
            decision,
            confidence: 1.0,
            probabilities: [(decision.as_str().to_owned(), 1.0)].into_iter().collect(),
            provider: "rule-based".to_owned(),
            model: "fixture".to_owned(),
            latency_ms: 0,
            fallback: false,
            fallback_reason: None,
        };
        ConversationTrace {
            core_state_before: ConversationCoreState::default(),
            invocation_gate: gate(Decision::Speak),
            preparation: None,
            assessments: Vec::new(),
            provider_candidates: Vec::new(),
            provider_selection: None,
            provider_telemetry: Default::default(),
            generation_latency_ms: 10,
            generated_candidates: candidates,
            language_provider_id: id.to_owned(),
            language_provider: "mock".to_owned(),
            language_model: "fixture".to_owned(),
            language_latency_ms: 10,
            response_gate: gate(Decision::Accept),
            candidate_digest: digest.to_owned(),
            retry_count: 0,
            core_state_after: ConversationCoreState::default(),
        }
    }

    #[test]
    fn selection_requires_final_id_digest_and_success() {
        let mut failed = candidate("primary", 1, "final");
        failed.error_code = Some("TIMEOUT".to_owned());
        let mut candidates = vec![
            candidate("primary", 0, "superseded"),
            candidate("other", 0, "final"),
            failed,
        ];
        let accepted = trace(Vec::new(), "primary", "final");
        assert_eq!(selected_candidate_index(&candidates, Some(&accepted)), None);
        candidates.push(candidate("primary", 1, "final"));
        assert_eq!(
            selected_candidate_index(&candidates, Some(&accepted)),
            Some(3)
        );
        assert_eq!(selected_candidate_index(&candidates, None), None);
    }

    #[test]
    fn identical_retry_marks_only_latest_matching_attempt() {
        let candidates = vec![
            candidate("primary", 0, "same"),
            candidate("primary", 1, "same"),
            candidate("other", 0, "same"),
        ];
        let mut accepted = trace(Vec::new(), "primary", "same");
        accepted.retry_count = 1;
        assert_eq!(
            selected_candidate_index(&candidates, Some(&accepted)),
            Some(1)
        );
        // Reselection can choose an unchanged organ instead of the retried one.
        accepted.language_provider_id = "other".to_owned();
        assert_eq!(
            selected_candidate_index(&candidates, Some(&accepted)),
            Some(2)
        );
    }

    #[test]
    fn failed_turn_keeps_its_report_and_clears_previous_success_on_send() {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let mut app = NativeApp::with_channels(commands, events);
        app.apply_ready(ReadyState {
            individual_id: "fixture-individual".to_owned(),
            session_id: "fixture-session".to_owned(),
            subject: "fixture-user".to_owned(),
            history: Vec::new(),
            connection: ConnectionSummary {
                jev_configured: false,
                jev_model: "rule-based".to_owned(),
                jev_base_url: "in-process".to_owned(),
                llm_provider: "mock".to_owned(),
                llm_model: "fixture".to_owned(),
                llm_base_url: "in-process".to_owned(),
                auth_variables: Vec::new(),
                language_providers: Vec::new(),
            },
        });
        let previous = vec![candidate("primary", 0, "previous")];
        event_tx
            .send(WorkerEvent::GenerationReport {
                candidates: previous.clone(),
                elapsed_ms: 10,
            })
            .unwrap();
        event_tx
            .send(WorkerEvent::Reply {
                response: "前の応答".to_owned(),
                trace: Some(Box::new(trace(previous, "primary", "previous"))),
                elapsed_ms: 20,
                history_messages: 2,
                core_state: None,
            })
            .unwrap();
        let ctx = egui::Context::default();
        app.poll_events(&ctx);
        assert!(app.trace.is_some());
        assert_eq!(app.last_elapsed_ms, Some(20));

        app.input = "次の入力".to_owned();
        app.send_current();
        assert!(matches!(
            command_rx.try_recv(),
            Ok(WorkerCommand::Send { .. })
        ));
        assert!(app.trace.is_none());
        assert!(app.generation_report.is_none());
        assert!(app.last_elapsed_ms.is_none());

        let mut failed = candidate("primary", 0, "unused");
        failed.response_bytes = None;
        failed.response_digest = None;
        failed.error_code = Some("TIMEOUT".to_owned());
        event_tx
            .send(WorkerEvent::GenerationReport {
                candidates: vec![failed],
                elapsed_ms: 60_000,
            })
            .unwrap();
        event_tx
            .send(WorkerEvent::ProvidersUpdated(Vec::new()))
            .unwrap();
        event_tx
            .send(WorkerEvent::Error {
                message: "turn failed".to_owned(),
            })
            .unwrap();
        app.poll_events(&ctx);
        let (candidates, elapsed_ms) = app.generation_details().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "primary");
        assert_eq!(candidates[0].error_code.as_deref(), Some("TIMEOUT"));
        assert_eq!(elapsed_ms, 60_000);
        assert_eq!(
            selected_candidate_index(candidates, app.trace.as_ref()),
            None
        );
        assert!(app.trace.is_none());
        assert!(app.last_elapsed_ms.is_none());
        assert!(!app.busy);
        assert_eq!(app.last_error.as_deref(), Some("turn failed"));
        assert_eq!(app.turn_count, 1);
        assert_eq!(app.messages.len(), 2);

        app.input = "再送".to_owned();
        app.send_current();
        assert!(app.generation_details().is_none());
        assert!(app.last_error.is_none());

        event_tx
            .send(WorkerEvent::Reply {
                response: "再送の応答".to_owned(),
                trace: Some(Box::new(trace(Vec::new(), "primary", "next"))),
                elapsed_ms: 20,
                history_messages: 4,
                core_state: None,
            })
            .unwrap();
        app.poll_events(&ctx);
        assert!(app.trace.is_some());
        // Even a locally rejected Send must not leave the prior success visible.
        app.input = "a".repeat(kamimusuhi_runtime::dialogue::MAX_INPUT_BYTES + 1);
        app.send_current();
        assert!(app.trace.is_none());
        assert!(app.generation_details().is_none());
        assert!(app.last_elapsed_ms.is_none());
        assert!(app.last_error.is_some());
        assert!(!app.busy);
    }
}
