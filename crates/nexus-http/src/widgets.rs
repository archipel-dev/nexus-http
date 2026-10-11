//! Shared widgets: template-highlighting text fields, a JSON code editor,
//! key/value tables, underline tab strips, pills and small vector icons.

use crate::theme::{Palette, mono};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Color32, CornerRadius, FontId, Layout, Pos2, Rect, Response, RichText, Sense, Stroke,
    TextEdit, Ui, Vec2, pos2, vec2,
};
use nexus_http_core::model::KeyValue;
use nexus_http_core::template::{self, SegmentKind};
use serde_json::{Map, Value};
use std::sync::Arc;

pub const ROW: f32 = 28.0;

// ------------------------------------------------------------------ highlighting

/// How plain text (outside template expressions) is colored.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Plain,
    Json,
}

/// Builds a layout job highlighting `{{ variables }}` and `{% tags %}`;
/// unknown variables are marked as errors.
pub fn highlight(
    text: &str,
    pal: &Palette,
    font: FontId,
    vars: &Map<String, Value>,
    syntax: Syntax,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for segment in template::tokenize(text) {
        let piece = &text[segment.span.clone()];
        match segment.kind {
            SegmentKind::Text => match syntax {
                Syntax::Plain => append(
                    &mut job,
                    piece,
                    font.clone(),
                    pal.text,
                    Color32::TRANSPARENT,
                ),
                Syntax::Json => json_tokens(&mut job, piece, pal, font.clone()),
            },
            SegmentKind::Variable(path) => {
                let known = template::lookup(vars, &path).is_some();
                let (fg, bg) = if known {
                    (pal.accent, pal.accent.gamma_multiply(0.18))
                } else {
                    (pal.error, pal.error.gamma_multiply(0.18))
                };
                append(&mut job, piece, font.clone(), fg, bg);
            }
            SegmentKind::Tag(_) => append(
                &mut job,
                piece,
                font.clone(),
                pal.purple,
                pal.purple.gamma_multiply(0.18),
            ),
            SegmentKind::Unclosed => append(
                &mut job,
                piece,
                font.clone(),
                pal.error,
                Color32::TRANSPARENT,
            ),
        }
    }
    job
}

fn append(job: &mut LayoutJob, text: &str, font: FontId, color: Color32, background: Color32) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            background,
            ..Default::default()
        },
    );
}

/// A tiny JSON lexer good enough for highlighting partial documents.
fn json_tokens(job: &mut LayoutJob, text: &str, pal: &Palette, font: FontId) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let color = if c == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            // A string followed by `:` is a key.
            let rest = text[i..].trim_start();
            if rest.starts_with(':') {
                pal.syntax_key
            } else {
                pal.syntax_string
            }
        } else if c == b'-' || c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_digit() || b"+-.eE".contains(&bytes[i])) {
                i += 1;
            }
            pal.syntax_number
        } else if c.is_ascii_alphabetic() {
            while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
            match &text[start..i] {
                "true" | "false" | "null" => pal.syntax_keyword,
                _ => pal.text,
            }
        } else if b"{}[],:".contains(&c) {
            i += 1;
            pal.syntax_punct
        } else {
            // Whitespace and anything else, up to the next interesting byte.
            i += 1;
            while i < bytes.len()
                && !(bytes[i] == b'"'
                    || bytes[i] == b'-'
                    || bytes[i].is_ascii_alphanumeric()
                    || b"{}[],:".contains(&bytes[i]))
            {
                i += 1;
            }
            pal.text
        };
        // Never split inside a UTF-8 sequence.
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
        append(
            job,
            &text[start..i],
            font.clone(),
            color,
            Color32::TRANSPARENT,
        );
    }
}

/// egui temp-data key holding the workspace's secret variable names, so
/// completion never previews their values.
pub const SECRET_KEYS_ID: &str = "nexus-secret-keys";

/// Single-line field with template highlighting. Returns the response.
pub fn template_field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    vars: &Map<String, Value>,
    width: f32,
    id: egui::Id,
) -> Response {
    let pal = Palette::of(ui);
    let font = FontId::proportional(13.0);
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| -> Arc<egui::Galley> {
        let mut job = highlight(buf.as_str(), &pal, font.clone(), vars, Syntax::Plain);
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    // Keys for an open completion popup are taken before the field sees them.
    let popup_id = id.with("completion");
    let open: bool = ui.data(|d| d.get_temp(popup_id.with("open")).unwrap_or(false));
    let (down, up, accept, dismiss) = if open {
        ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        })
    } else {
        (false, false, false, false)
    };
    let output = TextEdit::singleline(text)
        .id(id)
        .hint_text(RichText::new(hint).color(pal.faint))
        .desired_width(width)
        .frame(false)
        // While the popup is open Tab accepts a suggestion instead of moving focus.
        .lock_focus(open)
        .layouter(&mut layouter)
        .show(ui);
    let mut response = output.response.clone();
    let mut still_open = false;
    if response.has_focus()
        && !dismiss
        && let Some(range) = output.cursor_range
        && let Some(ctx) = crate::complete::context_at(text, range.primary.index)
    {
        let mut items = crate::complete::items(&ctx, vars);
        let secrets: Option<Arc<std::collections::HashSet<String>>> =
            ui.data(|d| d.get_temp(egui::Id::new(SECRET_KEYS_ID)));
        if let Some(secrets) = secrets {
            for item in &mut items {
                if secrets.contains(&item.label) {
                    item.detail = "••••••".into();
                }
            }
        }
        if !items.is_empty() {
            still_open = true;
            let sel_id = popup_id.with("sel");
            let mut sel: usize = ui.data(|d| d.get_temp(sel_id).unwrap_or(0));
            sel = sel.min(items.len() - 1);
            if down {
                sel = (sel + 1) % items.len();
            }
            if up {
                sel = (sel + items.len() - 1) % items.len();
            }
            let mut chosen = accept.then_some(sel);
            let area = egui::Area::new(popup_id.with("area"))
                .order(egui::Order::Foreground)
                .fixed_pos(response.rect.left_bottom() + vec2(-6.0, 6.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(pal.raised)
                        .stroke(pal.hairline())
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(4))
                        .shadow(egui::Shadow {
                            offset: [0, 4],
                            blur: 14,
                            spread: 0,
                            color: Color32::from_black_alpha(80),
                        })
                        .show(ui, |ui| {
                            for (i, item) in items.iter().enumerate() {
                                let (rect, row) =
                                    ui.allocate_exact_size(vec2(340.0, 24.0), Sense::click());
                                if i == sel || row.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(5),
                                        if i == sel { pal.selection } else { pal.hover },
                                    );
                                }
                                let color = if ctx.tag { pal.purple } else { pal.accent };
                                ui.painter().text(
                                    pos2(rect.left() + 8.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &item.label,
                                    mono(12.5),
                                    color,
                                );
                                ui.painter().text(
                                    pos2(rect.right() - 8.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    &item.detail,
                                    FontId::proportional(11.5),
                                    pal.weak,
                                );
                                if row.clicked() {
                                    chosen = Some(i);
                                }
                            }
                        });
                });
            let _ = area;
            ui.data_mut(|d| d.insert_temp(sel_id, sel));
            if let Some(i) = chosen {
                let (new_text, cursor) =
                    crate::complete::apply(text, &ctx, range.primary.index, &items[i]);
                *text = new_text;
                let mut state = output.state.clone();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(cursor),
                    )));
                state.store(ui.ctx(), id);
                ui.memory_mut(|m| m.request_focus(id));
                response.mark_changed();
                still_open = false;
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(popup_id.with("open"), still_open));
    response
}

/// Multi-line code editor with JSON or plain highlighting.
pub fn code_editor(
    ui: &mut Ui,
    text: &mut String,
    syntax: Syntax,
    vars: &Map<String, Value>,
    id: egui::Id,
) -> Response {
    let pal = Palette::of(ui);
    let font = mono(12.5);
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| -> Arc<egui::Galley> {
        let mut job = highlight(buf.as_str(), &pal, font.clone(), vars, syntax);
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let rows = (ui.available_height() / 17.0).max(4.0) as usize;
    egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                TextEdit::multiline(text)
                    .id(id)
                    .code_editor()
                    .frame(false)
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows)
                    .lock_focus(true)
                    .layouter(&mut layouter),
            )
        })
        .inner
}

/// Read-only highlighted text (response viewer).
pub fn code_view(ui: &mut Ui, text: &str, syntax: Syntax, id_salt: &str) {
    let mut pal = Palette::of(ui);
    // JSON value colors come from the kit theme (`parts::ValueKind`), like every data view.
    let kit = |kind: archipel_kit::ValueKind| kind.resolve(ui.ctx());
    pal.syntax_key = kit(archipel_kit::ValueKind::Key);
    pal.syntax_string = kit(archipel_kit::ValueKind::String);
    pal.syntax_number = kit(archipel_kit::ValueKind::Number);
    pal.syntax_keyword = kit(archipel_kit::ValueKind::Boolean);
    pal.syntax_punct = kit(archipel_kit::ValueKind::Punctuation);
    let vars = Map::new();
    egui::ScrollArea::both()
        .id_salt(id_salt)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Highlighting very large bodies would stall the frame; fall back to plain.
            let job = if text.len() > 512 * 1024 {
                let mut job = LayoutJob::default();
                append(&mut job, text, mono(12.5), pal.text, Color32::TRANSPARENT);
                job
            } else {
                highlight_plain_json(text, &pal, syntax, &vars)
            };
            let text_label = ui.add(egui::Label::new(job).selectable(true).extend());
            // The selectable label is focusable, so it needs an accessible name.
            text_label.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Body text")
            });
        });
}

fn highlight_plain_json(
    text: &str,
    pal: &Palette,
    syntax: Syntax,
    _vars: &Map<String, Value>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    match syntax {
        Syntax::Json => json_tokens(&mut job, text, pal, mono(12.5)),
        Syntax::Plain => append(&mut job, text, mono(12.5), pal.text, Color32::TRANSPARENT),
    }
    job
}

// ------------------------------------------------------------------ tables

/// Editable key/value table with enable checkboxes and a trailing blank row.
/// Returns true when anything changed.
pub fn kv_table(
    ui: &mut Ui,
    rows: &mut Vec<KeyValue>,
    id_salt: &str,
    name_hint: &str,
    vars: &Map<String, Value>,
) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    let mut remove = None;
    let width = ui.available_width();
    let check_w = 26.0;
    let del_w = 26.0;
    let col = ((width - check_w - del_w - 16.0) / 2.0).max(60.0);
    let count = rows.len();
    for i in 0..=count {
        let is_new = i == count;
        let mut scratch = KeyValue::new("", "");
        let row = if is_new { &mut scratch } else { &mut rows[i] };
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
        let hovered = ui.rect_contains_pointer(rect);
        if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(4), pal.hover.gamma_multiply(0.6));
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0_f32, pal.border.gamma_multiply(0.5)),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(4.0, 3.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        if is_new {
            child.add_space(check_w - 6.0 + child.spacing().item_spacing.x);
        } else if child
            .add_sized(
                [check_w - 6.0, 18.0],
                egui::Checkbox::without_text(&mut row.enabled),
            )
            .changed()
        {
            changed = true;
        }
        let id = ui.id().with(id_salt).with(i);
        let name_resp = template_field(
            &mut child,
            &mut row.name,
            if is_new { name_hint } else { "name" },
            vars,
            col,
            id.with("n"),
        );
        separator_v(&mut child, &pal);
        let value_resp =
            template_field(&mut child, &mut row.value, "value", vars, col, id.with("v"));
        if name_resp.changed() || value_resp.changed() {
            changed = true;
        }
        if !is_new
            && (hovered || name_resp.has_focus() || value_resp.has_focus())
            && icon_button(&mut child, Icon::Trash, pal.weak, "Delete").clicked()
        {
            remove = Some(i);
        }
        if is_new && (!scratch.name.is_empty() || !scratch.value.is_empty()) {
            rows.push(scratch);
            changed = true;
            // Keep typing in the new row's field.
            let focus = if name_resp.has_focus() {
                id.with("n")
            } else {
                id.with("v")
            };
            ui.memory_mut(|m| m.request_focus(focus));
        }
    }
    if let Some(i) = remove {
        rows.remove(i);
        changed = true;
    }
    changed
}

fn separator_v(ui: &mut Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 16.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0_f32, pal.border.gamma_multiply(0.6)),
    );
}

// ------------------------------------------------------------------ tabs & pills

/// Underline tab strip. `badge` returns an optional count shown next to a label.
pub fn tab_strip<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    tabs: &[T],
    label: impl Fn(T) -> String,
    badge: impl Fn(T) -> Option<String>,
) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for &tab in tabs {
            let selected = *current == tab;
            let text = label(tab);
            let badge_text = badge(tab);
            let font = FontId::proportional(13.0);
            let galley = ui.painter().layout_no_wrap(
                text.clone(),
                font.clone(),
                if selected { pal.strong } else { pal.weak },
            );
            let badge_galley = badge_text.map(|b| {
                ui.painter().layout_no_wrap(
                    b,
                    FontId::proportional(10.5),
                    if selected { pal.accent } else { pal.weak },
                )
            });
            let badge_w = badge_galley
                .as_ref()
                .map(|g| g.size().x + 10.0)
                .unwrap_or(0.0);
            let size = vec2(galley.size().x + 20.0 + badge_w, 32.0);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if resp.hovered() && !selected {
                ui.painter().rect_filled(
                    rect.shrink2(vec2(0.0, 5.0)),
                    CornerRadius::same(5),
                    pal.hover,
                );
            }
            let text_pos = pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(text_pos, galley.clone(), pal.text);
            if let Some(bg) = badge_galley {
                let r = Rect::from_min_size(
                    pos2(text_pos.x + galley.size().x + 6.0, rect.center().y - 8.0),
                    vec2(bg.size().x + 8.0, 16.0),
                );
                ui.painter().rect_filled(
                    r,
                    CornerRadius::same(8),
                    if selected {
                        pal.accent.gamma_multiply(0.2)
                    } else {
                        pal.hover
                    },
                );
                ui.painter().galley(
                    pos2(r.left() + 4.0, r.center().y - bg.size().y / 2.0),
                    bg,
                    pal.text,
                );
            }
            if selected {
                let line = Rect::from_min_max(
                    pos2(rect.left() + 6.0, rect.bottom() - 2.0),
                    pos2(rect.right() - 6.0, rect.bottom()),
                );
                ui.painter()
                    .rect_filled(line, CornerRadius::same(1), pal.accent);
            }
            if resp.clicked() && !selected {
                *current = tab;
                changed = true;
            }
        }
    });
    changed
}

/// Rounded label with a tinted background.
pub fn pill(ui: &mut Ui, text: &str, color: Color32, strong: bool) -> Response {
    let font = if strong {
        crate::theme::semibold(ui, 12.0)
    } else {
        FontId::proportional(12.0)
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size() + vec2(14.0, 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(5), color.gamma_multiply(0.16));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Bold method label in its color.
pub fn method_label(ui: &mut Ui, method: &nexus_http_core::model::Method, width: f32) {
    let pal = Palette::of(ui);
    let text = crate::theme::method_short(method);
    right_label(ui, &text, pal.method(method), width);
}

/// Method label, or `WS`/`SSE` for streaming requests.
pub fn request_label(ui: &mut Ui, request: &nexus_http_core::model::Request, width: f32) {
    use nexus_http_core::model::Protocol;
    let pal = Palette::of(ui);
    match request.protocol {
        Protocol::Http => method_label(ui, &request.method, width),
        Protocol::WebSocket => right_label(ui, "WS", pal.syntax_number, width),
        Protocol::Sse => right_label(ui, "SSE", pal.info, width),
        Protocol::Grpc => right_label(ui, "gRPC", pal.success, width),
    }
}

fn right_label(ui: &mut Ui, text: &str, color: Color32, width: f32) {
    let font = crate::theme::semibold(ui, 10.5);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    ui.painter().text(
        pos2(rect.right(), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        text,
        font,
        color,
    );
}

// ------------------------------------------------------------------ icons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Trash,
    Chevron(bool),
    Folder(bool),
    Gear,
    Cookie,
    Search,
    Import,
    Code,
    Close,
    Check,
    Globe,
    Dots,
}

/// Paints an icon centered in `rect`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = Stroke::new(1.3_f32, color);
    let p = |x: f32, y: f32| pos2(c.x + x, c.y + y);
    match icon {
        Icon::Plus => {
            painter.line_segment([p(-5.0, 0.0), p(5.0, 0.0)], s);
            painter.line_segment([p(0.0, -5.0), p(0.0, 5.0)], s);
        }
        Icon::Close => {
            painter.line_segment([p(-4.0, -4.0), p(4.0, 4.0)], s);
            painter.line_segment([p(4.0, -4.0), p(-4.0, 4.0)], s);
        }
        Icon::Check => {
            painter.add(egui::Shape::line(
                vec![p(-4.5, 0.5), p(-1.5, 3.5), p(4.5, -3.5)],
                Stroke::new(1.8_f32, color),
            ));
        }
        Icon::Trash => {
            painter.line_segment([p(-5.0, -3.5), p(5.0, -3.5)], s);
            painter.line_segment([p(-1.5, -5.5), p(1.5, -5.5)], s);
            painter.add(egui::Shape::closed_line(
                vec![p(-3.8, -3.5), p(-3.0, 5.5), p(3.0, 5.5), p(3.8, -3.5)],
                s,
            ));
        }
        Icon::Chevron(open) => {
            let pts = if open {
                vec![p(-3.5, -1.5), p(0.0, 2.0), p(3.5, -1.5)]
            } else {
                vec![p(-1.5, -3.5), p(2.0, 0.0), p(-1.5, 3.5)]
            };
            painter.add(egui::Shape::line(pts, s));
        }
        Icon::Folder(open) => {
            let body = Rect::from_min_max(p(-6.0, -3.0), p(6.0, 5.0));
            painter.rect_stroke(body, CornerRadius::same(2), s, egui::StrokeKind::Middle);
            painter.add(egui::Shape::line(
                vec![p(-6.0, -2.0), p(-6.0, -5.0), p(-1.5, -5.0), p(0.0, -3.0)],
                s,
            ));
            if open {
                painter.line_segment([p(-6.0, 0.0), p(6.0, 0.0)], s);
            }
        }
        Icon::Gear => {
            painter.circle_stroke(c, 2.2, s);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let (sin, cos) = a.sin_cos();
                painter.line_segment([p(cos * 4.2, sin * 4.2), p(cos * 6.0, sin * 6.0)], s);
            }
            painter.circle_stroke(c, 4.2, s);
        }
        Icon::Cookie => {
            painter.circle_stroke(c, 5.5, s);
            for (x, y) in [(-2.0, -1.5), (2.0, 1.0), (-0.5, 2.5), (1.5, -2.5)] {
                painter.circle_filled(p(x, y), 0.9, color);
            }
        }
        Icon::Search => {
            painter.circle_stroke(p(-1.0, -1.0), 4.0, s);
            painter.line_segment([p(2.0, 2.0), p(5.5, 5.5)], s);
        }
        Icon::Import => {
            painter.line_segment([p(0.0, -6.0), p(0.0, 2.0)], s);
            painter.add(egui::Shape::line(
                vec![p(-3.5, -1.5), p(0.0, 2.0), p(3.5, -1.5)],
                s,
            ));
            painter.add(egui::Shape::line(
                vec![p(-6.0, 2.0), p(-6.0, 5.5), p(6.0, 5.5), p(6.0, 2.0)],
                s,
            ));
        }
        Icon::Code => {
            painter.add(egui::Shape::line(
                vec![p(-2.5, -4.0), p(-6.0, 0.0), p(-2.5, 4.0)],
                s,
            ));
            painter.add(egui::Shape::line(
                vec![p(2.5, -4.0), p(6.0, 0.0), p(2.5, 4.0)],
                s,
            ));
        }
        Icon::Globe => {
            painter.circle_stroke(c, 5.5, s);
            painter.line_segment([p(-5.5, 0.0), p(5.5, 0.0)], s);
            painter.add(egui::Shape::ellipse_stroke(c, vec2(2.5, 5.5), s));
        }
        Icon::Dots => {
            for x in [-4.0, 0.0, 4.0] {
                painter.circle_filled(p(x, 0.0), 1.2, color);
            }
        }
    }
}

/// Square, flat icon button with hover background and tooltip.
pub fn icon_button(ui: &mut Ui, icon: Icon, color: Color32, tooltip: &str) -> Response {
    let pal = Palette::of(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), pal.hover);
    }
    paint_icon(
        ui.painter(),
        rect,
        icon,
        if resp.hovered() { pal.strong } else { color },
    );
    resp.on_hover_text(tooltip)
}

/// Primary (accent filled) button.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let pal = Palette::of(ui);
    let font = crate::theme::semibold(ui, 13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let size = vec2(galley.size().x + 28.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let mut fill = pal.accent;
    if !enabled {
        fill = fill.gamma_multiply(0.45);
    } else if resp.is_pointer_button_down_on() {
        fill = fill.gamma_multiply(0.85);
    } else if resp.hovered() {
        fill = brighten(fill);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    resp
}

/// Bordered secondary button.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    let pal = Palette::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), pal.text);
    let size = vec2(galley.size().x + 24.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if resp.hovered() {
        pal.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        fill,
        pal.hairline(),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, pal.text);
    resp
}

fn brighten(c: Color32) -> Color32 {
    let f = |v: u8| (v as u16 + (255 - v as u16) / 6) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// A rounded "field" frame used around inputs.
pub fn field_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.field)
        .stroke(pal.hairline())
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 5))
}

/// Left-aligned, dimmed form label of a fixed width.
pub fn form_label(ui: &mut Ui, text: &str, width: f32) {
    let pal = Palette::of(ui);
    ui.allocate_ui_with_layout(
        vec2(width, 22.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(text).color(pal.weak));
        },
    );
}

/// Section caption.
pub fn caption(ui: &mut Ui, text: &str) {
    let pal = Palette::of(ui);
    ui.label(RichText::new(text).size(11.5).color(pal.weak));
}

/// Centered empty state with a title, hint and optional action.
pub fn empty_state(ui: &mut Ui, title: &str, hint: &str) {
    let pal = Palette::of(ui);
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 3.0).max(12.0));
        ui.label(RichText::new(title).size(15.0).color(pal.strong));
        ui.add_space(6.0);
        ui.label(RichText::new(hint).color(pal.weak));
    });
}

/// Keyboard shortcut chip (`Ctrl` `Enter`).
pub fn shortcut(ui: &mut Ui, keys: &[&str]) {
    let pal = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for key in keys {
            let galley = ui.painter().layout_no_wrap(
                (*key).to_owned(),
                FontId::proportional(11.5),
                pal.weak,
            );
            let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(10.0, 6.0), Sense::hover());
            ui.painter().rect(
                rect,
                CornerRadius::same(4),
                pal.hover,
                Stroke::new(1.0_f32, pal.border),
                egui::StrokeKind::Inside,
            );
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, pal.weak);
        }
    });
}

/// Width of [`shortcut`] for `keys`.
pub fn shortcut_width(ui: &Ui, keys: &[&str]) -> f32 {
    keys.iter()
        .map(|k| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap((*k).to_owned(), FontId::proportional(11.5), Color32::WHITE)
                    .size()
                    .x
            }) + 10.0
        })
        .sum::<f32>()
        + 3.0 * keys.len().saturating_sub(1) as f32
}

/// A small filled dot (environment color).
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Parses `#rrggbb`.
pub fn parse_color(hex: &str) -> Option<Color32> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Paints a thin progress shimmer along the bottom of `rect` while busy.
pub fn busy_bar(ui: &Ui, rect: Rect, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let w = rect.width() * 0.25;
    let x = rect.left() + ((t * 0.9).fract() * (rect.width() + w)) - w;
    let bar = Rect::from_min_max(
        Pos2::new(x.max(rect.left()), rect.bottom() - 2.0),
        Pos2::new((x + w).min(rect.right()), rect.bottom()),
    );
    ui.painter().rect_filled(bar, CornerRadius::same(1), color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn highlights_known_and_unknown_variables() {
        let pal = Palette::from_visuals(&egui::Visuals::dark());
        let vars = json!({"host": "x"}).as_object().unwrap().clone();
        let job = highlight(
            "http://{{host}}/{{nope}}/{% uuid %}",
            &pal,
            mono(12.0),
            &vars,
            Syntax::Plain,
        );
        let colors: Vec<Color32> = job.sections.iter().map(|s| s.format.color).collect();
        assert!(colors.contains(&pal.accent));
        assert!(colors.contains(&pal.error));
        assert!(colors.contains(&pal.purple));
        assert_eq!(job.text, "http://{{host}}/{{nope}}/{% uuid %}");
    }

    #[test]
    fn json_lexer_covers_all_text_including_unicode() {
        let pal = Palette::from_visuals(&egui::Visuals::light());
        let text = "{\"name\": \"Zoë ✓\", \"n\": -1.5e3, \"ok\": true, \"x\": null}";
        let job = highlight(text, &pal, mono(12.0), &Map::new(), Syntax::Json);
        assert_eq!(job.text, text);
        let key = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "\"name\"")
            .unwrap();
        assert_eq!(key.format.color, pal.syntax_key);
        let kw = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "true")
            .unwrap();
        assert_eq!(kw.format.color, pal.syntax_keyword);
        // Unterminated strings must not panic.
        let _ = highlight("{\"a", &pal, mono(12.0), &Map::new(), Syntax::Json);
        let _ = highlight("\"é", &pal, mono(12.0), &Map::new(), Syntax::Json);
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#ff0000"), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(parse_color("red"), None);
    }
}
