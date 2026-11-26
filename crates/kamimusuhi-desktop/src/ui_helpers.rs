//! Shared UI helpers for `kamimusuhi-desktop`: toast feedback, lightweight
//! Markdown/code-block rendering, left-aligned navigation items, compact
//! timestamps, and UI zoom shortcuts.

use std::hash::Hash;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Stroke};

use crate::app::{AMBER, BG, BLUE, BORDER, GREEN, MUTED, PANEL, PANEL_RAISED, TEXT};

#[derive(Debug, Clone)]
pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) color: Color32,
    pub(crate) until: Instant,
}

impl Toast {
    pub(crate) fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            color: GREEN,
            until: Instant::now() + Duration::from_millis(1800),
        }
    }

    pub(crate) fn warn(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            color: AMBER,
            until: Instant::now() + Duration::from_millis(2400),
        }
    }
}

pub(crate) fn show_toast(ctx: &egui::Context, slot: &mut Option<Toast>) {
    let Some(toast) = slot.as_ref() else {
        return;
    };
    if Instant::now() >= toast.until {
        *slot = None;
        return;
    }
    let msg = toast.message.clone();
    let color = toast.color;
    egui::Area::new(egui::Id::new("desktop-toast"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-18.0, -18.0))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(PANEL_RAISED)
                .stroke(Stroke::new(1.0, color))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").small().color(color));
                        ui.label(RichText::new(msg).small().strong().color(TEXT));
                    });
                });
        });
    ctx.request_repaint_after(Duration::from_millis(120));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MdBlock {
    Heading(u8, String),
    Bullet(String),
    Paragraph(String),
    Code { lang: Option<String>, body: String },
}

pub(crate) fn parse_markdown(text: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut in_code = false;
    let mut code_lang: Option<String> = None;
    let mut code_lines: Vec<&str> = Vec::new();

    let flush_para = |para: &mut Vec<&str>, blocks: &mut Vec<MdBlock>| {
        if !para.is_empty() {
            blocks.push(MdBlock::Paragraph(para.join("\n")));
            para.clear();
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if in_code {
            if trimmed.starts_with("```") {
                blocks.push(MdBlock::Code {
                    lang: code_lang.take(),
                    body: code_lines.join("\n"),
                });
                code_lines.clear();
                in_code = false;
            } else {
                code_lines.push(line);
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("```") {
            flush_para(&mut para, &mut blocks);
            in_code = true;
            let lang = rest.trim();
            code_lang = (!lang.is_empty()).then(|| lang.to_owned());
            continue;
        }

        if trimmed.is_empty() {
            flush_para(&mut para, &mut blocks);
            continue;
        }

        let trimmed_start = line.trim_start();
        if let Some(h) = trimmed_start.strip_prefix("### ") {
            flush_para(&mut para, &mut blocks);
            blocks.push(MdBlock::Heading(3, h.trim().to_owned()));
        } else if let Some(h) = trimmed_start.strip_prefix("## ") {
            flush_para(&mut para, &mut blocks);
            blocks.push(MdBlock::Heading(2, h.trim().to_owned()));
        } else if let Some(h) = trimmed_start.strip_prefix("# ") {
            flush_para(&mut para, &mut blocks);
            blocks.push(MdBlock::Heading(1, h.trim().to_owned()));
        } else if let Some(b) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
            .or_else(|| trimmed_start.strip_prefix("• "))
        {
            flush_para(&mut para, &mut blocks);
            blocks.push(MdBlock::Bullet(b.trim().to_owned()));
        } else {
            para.push(line);
        }
    }

    if in_code {
        blocks.push(MdBlock::Code {
            lang: code_lang.take(),
            body: code_lines.join("\n"),
        });
    } else {
        flush_para(&mut para, &mut blocks);
    }
    blocks
}

pub(crate) fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = text;
    while let Some(pos) = [rest.find("https://"), rest.find("http://")]
        .into_iter()
        .flatten()
        .min()
    {
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| {
                !c.is_ascii()
                    || c.is_ascii_whitespace()
                    || matches!(c, '<' | '>' | '"' | '\'' | '`')
            })
            .unwrap_or(tail.len());
        let cleaned = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
        if cleaned.len() > "http://a".len() && !urls.iter().any(|u| u == cleaned) {
            urls.push(cleaned.to_owned());
        }
        rest = &tail[end.max(1)..];
    }
    urls
}

/// Renders lightweight Markdown blocks (headings, bullets, paragraphs, and
/// fenced code blocks with copy buttons) plus clickable URLs.
/// Returns a toast notice when a code block is copied.
pub(crate) fn render_markdown(ui: &mut egui::Ui, id_salt: impl Hash, text: &str) -> Option<String> {
    let mut copied = None;
    let blocks = parse_markdown(text);
    let block_count = blocks.len();
    for (idx, block) in blocks.into_iter().enumerate() {
        match block {
            MdBlock::Heading(level, title) => {
                let size = match level {
                    1 => 17.5,
                    2 => 15.5,
                    _ => 14.5,
                };
                ui.add(
                    egui::Label::new(RichText::new(title).strong().size(size).color(TEXT))
                        .wrap()
                        .selectable(true),
                );
            }
            MdBlock::Bullet(item) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("•").strong().color(BLUE));
                    ui.add(
                        egui::Label::new(RichText::new(item).color(TEXT))
                            .wrap()
                            .selectable(true),
                    );
                });
            }
            MdBlock::Paragraph(p) => {
                ui.add(
                    egui::Label::new(RichText::new(p).color(TEXT))
                        .wrap()
                        .selectable(true),
                );
            }
            MdBlock::Code { lang, body } => {
                egui::Frame::new()
                    .fill(BG)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(egui::CornerRadius::same(7))
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(lang.as_deref().unwrap_or("code"))
                                    .small()
                                    .monospace()
                                    .color(MUTED),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button(
                                            RichText::new("📋 コピー").small().color(MUTED),
                                        )
                                        .on_hover_text("コードをコピー")
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(body.clone());
                                        copied = Some("コードをコピーしました".to_owned());
                                    }
                                },
                            );
                        });
                        ui.add_space(3.0);
                        egui::ScrollArea::horizontal()
                            .id_salt((&id_salt, "code", idx))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&body).monospace().small().color(TEXT),
                                    )
                                    .selectable(true),
                                );
                            });
                    });
            }
        }
        if idx + 1 < block_count {
            ui.add_space(4.0);
        }
    }

    let urls = extract_urls(text);
    if !urls.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for url in urls.into_iter().take(4) {
                let label = if url.chars().count() > 48 {
                    let prefix: String = url.chars().take(45).collect();
                    format!("🔗 {prefix}…")
                } else {
                    format!("🔗 {url}")
                };
                ui.hyperlink_to(RichText::new(label).small().color(BLUE), &url);
            }
        });
    }
    copied
}

pub(crate) fn fmt_ts_compact(ts: &str) -> String {
    let trimmed = ts.trim();
    if let Some((date, rest)) = trimmed.split_once('T').or_else(|| trimmed.split_once(' ')) {
        let mut dp = date.split('-');
        let (_y, m, d) = (dp.next(), dp.next(), dp.next());
        let mut tp = rest.split(':');
        let (hh, mm) = (tp.next(), tp.next());
        if let (Some(m), Some(d), Some(hh), Some(mm)) = (m, d, hh, mm)
            && m.len() == 2
            && d.len() == 2
            && hh.len() == 2
            && mm.len() == 2
        {
            return format!("{m}/{d} {hh}:{mm}");
        }
    }
    trimmed.chars().take(16).collect()
}

pub(crate) fn nav_item(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    badge: Option<(String, Color32)>,
    shortcut: &str,
    selected: bool,
    compact: bool,
) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 34.0), egui::Sense::click());
    let fill = if selected {
        Color32::from_rgb(24, 62, 111)
    } else if resp.hovered() {
        Color32::from_rgb(19, 36, 56)
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        Stroke::new(1.0, Color32::from_rgb(46, 108, 185))
    } else {
        Stroke::NONE
    };
    let painter = ui.painter();
    painter.rect(rect, 7.0, fill, stroke, egui::StrokeKind::Inside);
    if selected {
        let bar = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 5.0), egui::vec2(3.0, 24.0));
        painter.rect_filled(bar, 2.0, BLUE);
    }

    let text_color = if let Some((_, b_col)) = &badge
        && *b_col == AMBER
        && !selected
    {
        AMBER
    } else if selected || resp.hovered() {
        TEXT
    } else {
        MUTED
    };

    if compact {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(16.0),
            text_color,
        );
        if let Some((_, b_col)) = badge {
            painter.circle_filled(rect.right_top() + egui::vec2(-6.0, 7.0), 3.5, b_col);
        }
    } else {
        let badge_w = badge.as_ref().map_or(0.0, |(b, _)| {
            (b.chars().count() as f32 * 7.5 + 18.0).max(26.0)
        });
        let label_clip = egui::Rect::from_min_max(
            rect.min + egui::vec2(10.0, 0.0),
            rect.max - egui::vec2(badge_w + 8.0, 0.0),
        );
        let galley = painter.layout_no_wrap(
            format!("{icon}  {label}"),
            egui::FontId::proportional(13.5),
            text_color,
        );
        let text_pos = egui::pos2(label_clip.left(), rect.center().y - galley.size().y * 0.5);
        ui.painter_at(label_clip)
            .galley(text_pos, galley, text_color);

        if let Some((b_text, b_col)) = badge {
            let b_rect = egui::Rect::from_center_size(
                egui::pos2(rect.right() - badge_w * 0.5 - 6.0, rect.center().y),
                egui::vec2(badge_w - 4.0, 20.0),
            );
            painter.rect(
                b_rect,
                10.0,
                b_col.gamma_multiply(0.18),
                Stroke::new(1.0, b_col.gamma_multiply(0.75)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                b_rect.center(),
                egui::Align2::CENTER_CENTER,
                b_text,
                egui::FontId::proportional(11.0),
                b_col,
            );
        }
    }

    let hover = if compact {
        format!("{label}  ({shortcut})")
    } else {
        format!("ショートカット: {shortcut}")
    };
    resp.on_hover_text(hover).clicked()
}

pub(crate) fn handle_zoom_shortcuts(ctx: &egui::Context, zoom: &mut f32) -> Option<String> {
    let mut next: Option<f32> = None;
    ctx.input(|i| {
        let cmd = i.modifiers.ctrl || i.modifiers.command;
        if !cmd {
            return;
        }
        if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
            next = Some((*zoom + 0.1).clamp(0.8, 1.4));
        } else if i.key_pressed(egui::Key::Minus) {
            next = Some((*zoom - 0.1).clamp(0.8, 1.4));
        } else if i.key_pressed(egui::Key::Num0) {
            next = Some(1.0);
        }
    });
    if let Some(new_zoom) = next
        && (new_zoom - *zoom).abs() > 0.001
    {
        *zoom = (new_zoom * 10.0).round() / 10.0;
        ctx.set_zoom_factor(*zoom);
        return Some(format!("表示倍率 {:.0}%", *zoom * 100.0));
    }
    None
}

pub(crate) fn apply_theme_base(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.faint_bg_color = BG;
    visuals.extreme_bg_color = BG;
    let radius = egui::CornerRadius::same(6);
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.fg_stroke.color = TEXT;
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.bg_fill = PANEL_RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.fg_stroke.color = MUTED;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(26, 51, 79);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 118, 178));
    visuals.widgets.hovered.fg_stroke.color = TEXT;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.bg_fill = Color32::from_rgb(29, 79, 138);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, BLUE);
    visuals.widgets.active.fg_stroke.color = TEXT;
    visuals.widgets.active.corner_radius = radius;
    visuals.widgets.open.bg_fill = PANEL_RAISED;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.open.corner_radius = radius;
    visuals.selection.bg_fill = BLUE;
    visuals.selection.stroke.color = Color32::WHITE;
    ctx.set_visuals(visuals);
    // Avoid double-scaling alongside `handle_zoom_shortcuts`.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_headings_bullets_and_code_blocks() {
        let input = "# Title\nIntro line\nsecond line\n\n- item one\n* item two\n\n```rust\nfn main() {}\n```";
        let blocks = parse_markdown(input);
        assert_eq!(
            blocks,
            vec![
                MdBlock::Heading(1, "Title".to_owned()),
                MdBlock::Paragraph("Intro line\nsecond line".to_owned()),
                MdBlock::Bullet("item one".to_owned()),
                MdBlock::Bullet("item two".to_owned()),
                MdBlock::Code {
                    lang: Some("rust".to_owned()),
                    body: "fn main() {}".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn unclosed_code_fence_is_preserved_as_code() {
        let blocks = parse_markdown("確認コマンド:\n```sh\ncargo test");
        assert_eq!(
            blocks,
            vec![
                MdBlock::Paragraph("確認コマンド:".to_owned()),
                MdBlock::Code {
                    lang: Some("sh".to_owned()),
                    body: "cargo test".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn extracts_urls_and_strips_trailing_punctuation() {
        let text = "詳細はこちら (https://example.com/a/b?x=1)、または http://127.0.0.1:7860/status。重複 https://example.com/a/b?x=1";
        assert_eq!(
            extract_urls(text),
            vec![
                "https://example.com/a/b?x=1".to_owned(),
                "http://127.0.0.1:7860/status".to_owned(),
            ]
        );
    }

    #[test]
    fn formats_timestamps_compactly_and_safely() {
        assert_eq!(fmt_ts_compact("2026-11-25T10:05:31.123Z"), "11/25 10:05");
        assert_eq!(fmt_ts_compact("2026-01-02 09:04:00"), "01/02 09:04");
        assert_eq!(fmt_ts_compact("短い日本語時刻"), "短い日本語時刻");
        assert_eq!(fmt_ts_compact(""), "");
    }
}
