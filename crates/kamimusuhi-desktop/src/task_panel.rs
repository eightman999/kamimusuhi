//! Task Plane panel: external agent executors, their tasks, the usage ledger
//! and per-model statistics, as served by the resident's `/v1/agents`.
//!
//! Every field of the panel document is optional on the wire; missing values
//! render as "—" and unknown costs as "不明" (never as zero).

use std::sync::mpsc::Sender;

use eframe::egui::{self, Color32, RichText, Stroke};
use serde_json::{Value, json};

use crate::app::{AMBER, BLUE, BORDER, GREEN, MUTED, PANEL, PANEL_RAISED, TEXT};
use crate::remote_app::{RED, VIOLET, chip};
use kamimusuhi_resident::remote::RemoteCommand;

const DASH: &str = "—";
const UNKNOWN: &str = "不明";
const DETAIL_W: f32 = 440.0;

/// Detail keys shown first, in this order; any other key follows.
const DETAIL_KEYS: [(&str, &str); 13] = [
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
];

#[derive(Default)]
pub(crate) struct AgentsPanel {
    /// Last panel document; `None` until the first fetch.
    panel: Option<Value>,
    /// The node answered 404: it has no task plane.
    absent: bool,
    selected: Option<String>,
    running_only: bool,
    instruction: String,
    reply: Option<String>,
}

impl AgentsPanel {
    pub(crate) fn set_panel(&mut self, panel: Value) {
        if panel.is_null() {
            self.absent = true;
            self.panel = None;
        } else {
            self.absent = false;
            self.panel = Some(panel);
        }
    }

    pub(crate) fn set_reply(&mut self, reply: Value) {
        let text = reply["message"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| reply.to_string());
        self.reply = Some(clip(&text, 300));
    }

    /// Tasks currently in progress (for the nav badge).
    pub(crate) fn running(&self) -> usize {
        self.tasks()
            .iter()
            .filter(|t| t["status"] == "in_progress")
            .count()
    }

    fn tasks(&self) -> Vec<&Value> {
        self.panel
            .as_ref()
            .and_then(|p| p["tasks"].as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, commands: &Sender<RemoteCommand>) {
        let send = |body: Value| {
            let _ = commands.send(RemoteCommand::Agents(body));
        };
        ui.horizontal(|ui| {
            ui.heading(RichText::new("外部エージェント").color(TEXT));
            if let Some(p) = &self.panel {
                let h = &p["health"];
                ui.label(
                    RichText::new(format!(
                        "実行中 {}/{} · 待機含む {} · 経路 {}",
                        num_or_dash(&h["running"]),
                        num_or_dash(&h["max_concurrent"]),
                        num_or_dash(&h["queued_or_running"]),
                        text_or_dash(&h["routing_mode"]),
                    ))
                    .small()
                    .color(MUTED),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("↻ 再検査").clicked() {
                    send(json!({"action": "refresh"}));
                }
                ui.add_space(10.0);
                ui.selectable_value(&mut self.running_only, false, "すべて");
                ui.selectable_value(&mut self.running_only, true, "実行中のみ");
            });
        });
        ui.separator();
        if self.absent {
            ui.add_space(20.0);
            ui.label(RichText::new("この node には Task Plane がありません").color(MUTED));
            return;
        }
        let Some(panel) = self.panel.clone() else {
            ui.label(RichText::new("読み込み中…").color(MUTED));
            return;
        };
        if let Some(e) = panel["health"]["config_error"]
            .as_str()
            .filter(|e| !e.is_empty())
        {
            ui.label(RichText::new(format!("設定エラー: {e}")).small().color(RED));
        }
        executor_strip(ui, &panel["health"]["executors"]);
        if let Some(r) = &self.reply {
            ui.label(
                RichText::new(format!("直近の操作: {r}"))
                    .small()
                    .color(MUTED),
            );
        }
        ui.add_space(6.0);

        let mut tasks: Vec<&Value> = panel["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| !self.running_only || is_active(t))
            .collect();
        tasks.sort_by(|a, b| sort_key(b).cmp(sort_key(a)));
        if self
            .selected
            .as_deref()
            .is_some_and(|id| !tasks.iter().any(|t| t["id"] == id))
        {
            self.selected = None;
        }
        let detail_w = if self.selected.is_some() {
            DETAIL_W
        } else {
            0.0
        };
        let list_w = (ui.available_width() - detail_w - 12.0).max(300.0);
        let top_down = egui::Layout::top_down(egui::Align::Min);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(list_w, ui.available_height()), top_down, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("agents-list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if tasks.is_empty() {
                            ui.label(RichText::new("表示するタスクはありません").color(MUTED));
                        }
                        for t in &tasks {
                            if task_row(ui, t, self.selected.as_deref()) {
                                let id = t["id"].as_str().map(str::to_owned);
                                self.selected = if self.selected == id { None } else { id };
                                self.instruction.clear();
                            }
                        }
                        ui.add_space(10.0);
                        ledger_card(ui, &panel["ledger"]);
                        ui.add_space(10.0);
                        stats_card(ui, &panel["stats"]);
                    });
            });
            if let Some(id) = self.selected.clone()
                && let Some(t) = tasks.iter().find(|t| t["id"] == id.as_str())
            {
                ui.add_space(12.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(DETAIL_W, ui.available_height()),
                    top_down,
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("agents-detail")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if self.detail(ui, t, &send) {
                                    self.selected = None;
                                }
                            });
                    },
                );
            }
        });
    }

    /// Returns true when the detail asks to be closed.
    fn detail(&mut self, ui: &mut egui::Ui, t: &Value, send: &dyn Fn(Value)) -> bool {
        let id = t["id"].as_str().unwrap_or("");
        let status = t["status"].as_str().unwrap_or("");
        let mut close = false;
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    chip(ui, status_label(status), status_color(status));
                    ui.label(RichText::new(id).small().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.small_button("閉じる").clicked() {
                            close = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(t["title"].as_str().unwrap_or(DASH))
                        .strong()
                        .size(16.0)
                        .color(TEXT),
                );
                ui.add_space(6.0);

                // Actions.
                ui.horizontal_wrapped(|ui| {
                    if is_active(t)
                        && ui
                            .add(
                                egui::Button::new(RichText::new("取消").color(Color32::WHITE))
                                    .fill(RED),
                            )
                            .clicked()
                    {
                        send(json!({"action": "cancel", "id": id}));
                    }
                    if is_finished(t) {
                        let current = t["result"]["feedback"].as_str();
                        for (verdict, label, color) in
                            [("accept", "採用", GREEN), ("reject", "不採用", RED)]
                        {
                            let chosen = current == Some(verdict);
                            let text = if chosen {
                                RichText::new(format!("✔ {label}")).color(Color32::BLACK)
                            } else {
                                RichText::new(label).color(color)
                            };
                            let mut button =
                                egui::Button::new(text).stroke(Stroke::new(1.0, color));
                            if chosen {
                                button = button.fill(color);
                            }
                            if ui.add(button).clicked() {
                                send(json!({"action": "feedback", "id": id, "verdict": verdict}));
                            }
                        }
                    }
                });
                if is_finished(t)
                    && t["result"]["session_id"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
                {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.instruction)
                            .hint_text("同じセッションで続けて依頼する内容")
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    if ui
                        .add(
                            egui::Button::new(RichText::new("続けて依頼").color(Color32::WHITE))
                                .fill(BLUE),
                        )
                        .clicked()
                        && !self.instruction.trim().is_empty()
                    {
                        send(json!({"action": "continue", "id": id,
                            "instruction": self.instruction.trim()}));
                        self.instruction.clear();
                    }
                }
                ui.add_space(8.0);

                section(ui, "概要");
                let mut rows: Vec<(String, String)> = vec![
                    ("担当".into(), text_or_dash(&t["owner"])),
                    ("作成".into(), timestamp(&t["created"])),
                    ("更新".into(), timestamp(&t["updated"])),
                    ("終了".into(), timestamp(&t["finished"])),
                    ("所要".into(), fmt_duration(t["duration_secs"].as_f64())),
                    ("依存".into(), list_or_dash(&t["depends_on"])),
                ];
                let detail = &t["detail"];
                for (k, label) in DETAIL_KEYS {
                    rows.push((label.into(), text_or_dash(&detail[k])));
                }
                for (k, v) in detail.as_object().into_iter().flatten() {
                    if !DETAIL_KEYS.iter().any(|(key, _)| key == k) {
                        rows.push((k.clone(), text_or_dash(v)));
                    }
                }
                kv_grid(ui, ("agent-kv", id), &rows);

                ui.add_space(8.0);
                section(ui, "経過メモ");
                let notes: Vec<&Value> = t["notes"].as_array().into_iter().flatten().collect();
                if notes.is_empty() {
                    ui.label(RichText::new(DASH).small().color(MUTED));
                }
                for n in notes.iter().rev() {
                    ui.label(
                        RichText::new(format!(
                            "{} [{}] {}",
                            timestamp(&n["at"]),
                            n["by"].as_str().unwrap_or(DASH),
                            n["text"].as_str().unwrap_or("")
                        ))
                        .small()
                        .color(TEXT),
                    );
                }

                ui.add_space(8.0);
                section(ui, "結果");
                result_block(ui, id, &t["result"]);
            });
        close
    }
}

// ── pieces ─────────────────────────────────────────────────────────────────

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).small().strong().color(MUTED));
}

fn kv_grid(ui: &mut egui::Ui, id: impl std::hash::Hash, rows: &[(String, String)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (k, v) in rows {
                ui.label(RichText::new(k).small().color(MUTED));
                ui.add(
                    egui::Label::new(RichText::new(v).small().color(TEXT))
                        .wrap()
                        .selectable(true),
                );
                ui.end_row();
            }
        });
}

fn executor_strip(ui: &mut egui::Ui, executors: &Value) {
    let list: Vec<&Value> = executors.as_array().into_iter().flatten().collect();
    if list.is_empty() {
        ui.label(
            RichText::new("実行者が設定されていません")
                .small()
                .color(MUTED),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for e in list {
            let (state, color) = executor_state(e);
            egui::Frame::new()
                .fill(PANEL_RAISED)
                .stroke(Stroke::new(1.0, color.gamma_multiply(0.7)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.set_width(220.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(color));
                        ui.label(RichText::new(text_or_dash(&e["name"])).strong().color(TEXT));
                        ui.label(RichText::new(state).small().color(color));
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} / {} · 実行 {}/{}",
                            text_or_dash(&e["adapter"]),
                            text_or_dash(&e["protocol"]),
                            num_or_dash(&e["running"]),
                            num_or_dash(&e["max_concurrent"]),
                        ))
                        .small()
                        .color(MUTED),
                    );
                    let models = match &e["models"] {
                        Value::Array(a) => a.len().to_string(),
                        Value::Object(o) => o.len().to_string(),
                        other => num_or_dash(other),
                    };
                    ui.label(
                        RichText::new(format!(
                            "モデル {models} · 既定 {} · {}",
                            text_or_dash(&e["default_model"]),
                            text_or_dash(&e["default_billing"]),
                        ))
                        .small()
                        .color(MUTED),
                    );
                    ui.label(
                        RichText::new(format!("harness {}", text_or_dash(&e["harness_version"])))
                            .small()
                            .color(MUTED),
                    );
                    let sup = &e["supervisor"];
                    if sup.is_object() {
                        let st = sup["state"].as_str().unwrap_or(DASH);
                        let color = match st {
                            "running" => GREEN,
                            "starting" => AMBER,
                            "failed" => RED,
                            _ => MUTED,
                        };
                        let conns = sup["connections"]
                            .as_u64()
                            .map(|n| format!(" · 接続 {n}"))
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(format!(
                                "監督 {} {st}{conns}",
                                text_or_dash(&sup["kind"])
                            ))
                            .small()
                            .color(color),
                        )
                        .on_hover_text(text_or_dash(&sup["detail"]));
                    }
                    for (key, prefix) in [("detail", ""), ("catalog_error", "カタログ: ")] {
                        if let Some(msg) = e[key].as_str().filter(|s| !s.is_empty()) {
                            ui.label(
                                RichText::new(format!("{prefix}{}", clip(msg, 90)))
                                    .small()
                                    .color(if key == "catalog_error" { RED } else { MUTED }),
                            )
                            .on_hover_text(msg);
                        }
                    }
                });
        }
    });
}

/// Returns true when the row was clicked.
fn task_row(ui: &mut egui::Ui, t: &Value, selected: Option<&str>) -> bool {
    let id = t["id"].as_str().unwrap_or("");
    let status = t["status"].as_str().unwrap_or("");
    let color = status_color(status);
    let is_selected = selected == Some(id);
    let d = &t["detail"];
    let inner = egui::Frame::new()
        .fill(if is_selected { PANEL_RAISED } else { PANEL })
        .stroke(Stroke::new(
            1.0,
            if is_selected { Color32::WHITE } else { BORDER },
        ))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(color));
                ui.label(RichText::new(status_label(status)).small().color(color));
                ui.add(
                    egui::Label::new(
                        RichText::new(t["title"].as_str().unwrap_or(DASH))
                            .strong()
                            .color(TEXT),
                    )
                    .truncate(),
                );
            });
            let mut meta = format!(
                "{} · {} · {} · {} · ツール {}",
                text_or_dash(&d["executor"]),
                text_or_dash(&d["model"]),
                text_or_dash(&d["billing"]),
                fmt_duration(t["duration_secs"].as_f64()),
                num_or_dash(&d["tool_calls"]),
            );
            if t["result"]["read_only_violation"].as_bool() == Some(true) {
                meta.push_str(" · ⚠ 読み取り専用違反");
            }
            ui.label(RichText::new(meta).small().color(MUTED));
        });
    ui.add_space(4.0);
    ui.interact(
        inner.response.rect,
        egui::Id::new(("agent-task", id)),
        egui::Sense::click(),
    )
    .clicked()
}

fn result_block(ui: &mut egui::Ui, id: &str, result: &Value) {
    if !result.is_object() {
        ui.label(RichText::new("結果はまだありません").small().color(MUTED));
        return;
    }
    if result["read_only_violation"].as_bool() == Some(true) {
        ui.label(
            RichText::new("⚠ 読み取り専用のはずのタスクがファイルを変更しました")
                .strong()
                .color(RED),
        );
    }
    if let Some(e) = result["error"].as_str().filter(|e| !e.is_empty()) {
        ui.add(
            egui::Label::new(RichText::new(format!("エラー: {e}")).small().color(RED))
                .wrap()
                .selectable(true),
        );
    }
    let summary = result["summary"].as_str().unwrap_or("");
    egui::Frame::new()
        .fill(PANEL_RAISED)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(8.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("agent-summary", id))
                .max_height(240.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(if summary.is_empty() { DASH } else { summary })
                                .color(TEXT),
                        )
                        .wrap()
                        .selectable(true),
                    );
                });
        });
    ui.add_space(6.0);
    let cost = &result["cost"];
    let rows: Vec<(String, String)> = vec![
        ("使用量".into(), fmt_usage(&result["usage"])),
        (
            "費用".into(),
            format!(
                "報告 {} · 推定 {} · {}",
                fmt_usd(&cost["reported_usd"]),
                fmt_usd(&cost["estimated_usd"]),
                text_or_dash(&cost["billing"])
            ),
        ),
        (
            "変更ファイル".into(),
            list_or_dash(&result["files_changed"]),
        ),
        ("ツール活動".into(), list_or_dash(&result["tool_activity"])),
        ("セッション".into(), text_or_dash(&result["session_id"])),
        ("証跡".into(), text_or_dash(&result["evidence_id"])),
        ("評価".into(), text_or_dash(&result["eval"])),
        (
            "フィードバック".into(),
            match result["feedback"].as_str() {
                Some("accept") => "採用".into(),
                Some("reject") => "不採用".into(),
                _ => text_or_dash(&result["feedback"]),
            },
        ),
    ];
    kv_grid(ui, ("agent-result", id), &rows);
}

fn ledger_card(ui: &mut egui::Ui, ledger: &Value) {
    crate::app::card(ui, "利用台帳", |ui| {
        if !ledger.is_object() {
            ui.label(RichText::new(DASH).small().color(MUTED));
            return;
        }
        ui.label(
            RichText::new(format!(
                "今日 {} · 今月 {}",
                text_or_dash(&ledger["day"]),
                text_or_dash(&ledger["month"])
            ))
            .small()
            .color(MUTED),
        );
        let (today, month, quotas) = (&ledger["today"], &ledger["this_month"], &ledger["quotas"]);
        let mut names: Vec<String> = [today, month, quotas]
            .iter()
            .flat_map(|v| v.as_object().into_iter().flatten().map(|(k, _)| k.clone()))
            .filter(|k| k != "_total")
            .collect();
        names.sort();
        names.dedup();
        if names.is_empty() && quotas.get("_total").is_none() {
            ui.label(RichText::new("記録はまだありません").small().color(MUTED));
            return;
        }
        let total = quotas.get("_total").is_some() || names.len() > 1;
        egui::Grid::new("agent-ledger")
            .num_columns(6)
            .spacing([14.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for h in [
                    "実行者",
                    "今日 件数",
                    "今日 費用",
                    "今日 tokens",
                    "今月 件数",
                    "今月 費用",
                ] {
                    ui.label(RichText::new(h).small().strong().color(MUTED));
                }
                ui.end_row();
                let rows = names
                    .iter()
                    .map(|n| (n.as_str(), today[n].clone(), month[n].clone()))
                    .chain(total.then(|| {
                        (
                            "_total",
                            today
                                .get("_total")
                                .cloned()
                                .unwrap_or_else(|| sum_usage(today)),
                            month
                                .get("_total")
                                .cloned()
                                .unwrap_or_else(|| sum_usage(month)),
                        )
                    }))
                    .collect::<Vec<_>>();
                for (name, d, m) in rows {
                    let q = &quotas[name];
                    let label = if name == "_total" { "合計" } else { name };
                    ui.label(RichText::new(label).small().strong().color(TEXT));
                    for cell in [
                        fmt_count_quota(&d["tasks"], &q["max_tasks_per_day"]),
                        fmt_usd_quota(&d["usd"], &d["unpriced"], &q["max_usd_per_day"]),
                        fmt_tokens_quota(&d["tokens"], &q["max_tokens_per_day"]),
                        fmt_count_quota(&m["tasks"], &q["max_tasks_per_month"]),
                        fmt_usd_quota(&m["usd"], &m["unpriced"], &q["max_usd_per_month"]),
                    ] {
                        ui.label(RichText::new(cell).small().color(TEXT));
                    }
                    ui.end_row();
                }
            });
    });
}

fn stats_card(ui: &mut egui::Ui, stats: &Value) {
    crate::app::card(ui, "実績（モデル × ハーネス）", |ui| {
        let rows: Vec<&Value> = stats.as_array().into_iter().flatten().collect();
        if rows.is_empty() {
            ui.label(RichText::new("実績はまだありません").small().color(MUTED));
            return;
        }
        egui::ScrollArea::horizontal()
            .id_salt("agent-stats")
            .show(ui, |ui| {
                egui::Grid::new("agent-stats-grid")
                    .num_columns(9)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for h in [
                            "種類",
                            "実行者",
                            "モデル",
                            "n",
                            "成功",
                            "採用/不採用",
                            "中央値",
                            "平均費用",
                            "score",
                        ] {
                            ui.label(RichText::new(h).small().strong().color(MUTED));
                        }
                        ui.end_row();
                        for s in rows {
                            for cell in [
                                text_or_dash(&s["task_kind"]),
                                text_or_dash(&s["executor"]),
                                text_or_dash(&s["model"]),
                                num_or_dash(&s["n"]),
                                fmt_success(&s["succeeded"], &s["n"]),
                                format!(
                                    "{}/{}",
                                    num_or_dash(&s["accepted"]),
                                    num_or_dash(&s["rejected"])
                                ),
                                fmt_duration(s["median_ms"].as_f64().map(|ms| ms / 1000.0)),
                                fmt_usd(&s["mean_usd"]),
                                s["score"]
                                    .as_f64()
                                    .map_or_else(|| DASH.to_owned(), |x| format!("{x:.2}")),
                            ] {
                                ui.label(RichText::new(cell).small().color(TEXT));
                            }
                            ui.end_row();
                        }
                    });
            });
    });
}

// ── pure helpers ───────────────────────────────────────────────────────────

fn is_active(t: &Value) -> bool {
    matches!(t["status"].as_str(), Some("waiting" | "in_progress"))
}

fn is_finished(t: &Value) -> bool {
    matches!(t["status"].as_str(), Some("done" | "failed" | "cancelled"))
}

fn sort_key(t: &Value) -> &str {
    t["created"]
        .as_str()
        .or_else(|| t["updated"].as_str())
        .unwrap_or("")
}

fn status_label(status: &str) -> &str {
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

fn status_color(status: &str) -> Color32 {
    match status {
        "waiting" => VIOLET,
        "in_progress" => BLUE,
        "done" => GREEN,
        "failed" => RED,
        _ => MUTED,
    }
}

fn executor_state(e: &Value) -> (&'static str, Color32) {
    if e["enabled"].as_bool() == Some(false) {
        ("無効", MUTED)
    } else if e["ok"].as_bool() == Some(true) {
        ("OK", GREEN)
    } else {
        ("未検出", RED)
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(max_chars).collect();
        s.push('…');
        s
    }
}

/// Any JSON value as display text; null / missing / empty → "—".
fn text_or_dash(v: &Value) -> String {
    match v {
        Value::Null => DASH.to_owned(),
        Value::String(s) if s.is_empty() => DASH.to_owned(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "はい" } else { "いいえ" }.to_owned(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn num_or_dash(v: &Value) -> String {
    if v.is_number() {
        v.to_string()
    } else {
        DASH.to_owned()
    }
}

fn list_or_dash(v: &Value) -> String {
    match v.as_array() {
        Some(a) if !a.is_empty() => a.iter().map(text_or_dash).collect::<Vec<_>>().join("\n"),
        Some(_) => DASH.to_owned(),
        None => text_or_dash(v),
    }
}

fn timestamp(v: &Value) -> String {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map_or_else(|| DASH.to_owned(), |s| s.replace('T', " "))
}

/// Dollar amount; a missing / null price is unknown, never zero.
fn fmt_usd(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x == 0.0 || x.abs() >= 0.01 => format!("${x:.2}"),
        Some(x) => format!("${x:.4}"),
        None => UNKNOWN.to_owned(),
    }
}

fn fmt_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn fmt_usage(usage: &Value) -> String {
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

fn fmt_duration(secs: Option<f64>) -> String {
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

fn fmt_count_quota(used: &Value, max: &Value) -> String {
    let used = used.as_u64().unwrap_or(0);
    match max.as_u64() {
        Some(max) => format!("{used}/{max} 件"),
        None => format!("{used} 件"),
    }
}

fn fmt_usd_quota(used: &Value, unpriced: &Value, max: &Value) -> String {
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

fn fmt_tokens_quota(used: &Value, max: &Value) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_cost_is_never_zero() {
        assert_eq!(fmt_usd(&Value::Null), "不明");
        assert_eq!(fmt_usd(&json!({})["reported_usd"]), "不明");
        assert_eq!(fmt_usd(&json!(0.0)), "$0.00");
        assert_eq!(fmt_usd(&json!(0.1234)), "$0.12");
        assert_eq!(fmt_usd(&json!(0.0042)), "$0.0042");
    }

    #[test]
    fn usage_and_durations_render_missing_as_dash() {
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
    }

    #[test]
    fn ledger_quota_formatting() {
        assert_eq!(fmt_count_quota(&json!(3), &json!(20)), "3/20 件");
        assert_eq!(fmt_count_quota(&json!(3), &Value::Null), "3 件");
        assert_eq!(
            fmt_usd_quota(&json!(0.12), &json!(0), &json!(1.0)),
            "$0.12/$1.00"
        );
        assert_eq!(
            fmt_usd_quota(&json!(0.0), &json!(2), &Value::Null),
            "$0.00 (+2 件不明)"
        );
        assert_eq!(
            fmt_tokens_quota(&json!(1500), &json!(100_000)),
            "1.5k/100.0k"
        );
        let total = sum_usage(&json!({
            "a": {"tasks": 2, "usd": 0.5, "tokens": 10, "unpriced": 1},
            "b": {"tasks": 1, "usd": 0.25}
        }));
        assert_eq!(total["tasks"], 3);
        assert_eq!(total["unpriced"], 1);
        assert_eq!(fmt_usd(&total["usd"]), "$0.75");
    }
}
