//! GraphQL query editor (highlighting + schema-aware completion) and the
//! schema explorer.

use crate::theme::{Palette, mono};
use crate::widgets::{self, Syntax};
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, RichText, Sense, TextEdit, Ui, pos2, vec2};
use nexus_http_core::graphql::{Field, Schema};
use serde_json::{Map, Value};
use std::sync::Arc;

/// GraphQL syntax highlighting (keywords, variables, strings, comments).
pub fn highlight(text: &str, pal: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font = mono(12.5);
    let bytes = text.as_bytes();
    let mut i = 0;
    let push = |job: &mut LayoutJob, s: &str, c: Color32| {
        job.append(
            s,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: c,
                ..Default::default()
            },
        );
    };
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let color = if c == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            pal.faint
        } else if c == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            pal.syntax_string
        } else if c == b'$' || c == b'@' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if c == b'$' {
                pal.syntax_number
            } else {
                pal.purple
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let next = text[i..].trim_start();
            match word {
                "query" | "mutation" | "subscription" | "fragment" | "on" | "true" | "false"
                | "null" => pal.syntax_keyword,
                _ if next.starts_with(':') => pal.syntax_key,
                _ if word.chars().next().is_some_and(char::is_uppercase) => pal.info,
                _ => pal.text,
            }
        } else if c.is_ascii_digit() || c == b'-' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            pal.syntax_number
        } else {
            i += 1;
            while i < bytes.len() && !text.is_char_boundary(i) {
                i += 1;
            }
            if b"{}()[]:!=".contains(&c) {
                pal.syntax_punct
            } else {
                pal.text
            }
        };
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
        push(&mut job, &text[start..i.min(text.len())], color);
    }
    job
}

/// Query editor with completion from `schema`. Returns whether the text changed.
pub fn query_editor(ui: &mut Ui, text: &mut String, schema: Option<&Schema>, id: egui::Id) -> bool {
    let pal = Palette::of(ui);
    let popup = id.with("gql-completion");
    let open: bool = ui.data(|d| d.get_temp(popup.with("open")).unwrap_or(false));
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
    let explicit = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| -> Arc<egui::Galley> {
        let mut job = highlight(buf.as_str(), &pal);
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let rows = (ui.available_height() / 17.0).max(4.0) as usize;
    let output = egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            TextEdit::multiline(text)
                .id(id)
                .code_editor()
                .frame(false)
                .desired_width(f32::INFINITY)
                .desired_rows(rows)
                .lock_focus(true)
                .layouter(&mut layouter)
                .show(ui)
        })
        .inner;
    let mut changed = output.response.changed();
    let mut still_open = false;
    if let (Some(schema), true, false) = (schema, output.response.has_focus(), dismiss)
        && let Some(range) = output.cursor_range
    {
        let char_index = range.primary.index;
        let byte = text
            .char_indices()
            .nth(char_index)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        let typing = text[..byte]
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let wanted = (typing && (open || changed)) || explicit || open;
        if wanted
            && let Some((start, fields)) = schema.complete(text, byte)
            && !fields.is_empty()
        {
            still_open = true;
            let sel_id = popup.with("sel");
            let mut sel: usize = ui.data(|d| d.get_temp(sel_id).unwrap_or(0));
            sel = sel.min(fields.len() - 1);
            if down {
                sel = (sel + 1) % fields.len();
            }
            if up {
                sel = (sel + fields.len() - 1) % fields.len();
            }
            let caret = output.galley.pos_from_cursor(range.primary);
            let anchor = output.galley_pos + caret.left_bottom().to_vec2() + vec2(0.0, 4.0);
            let mut chosen = accept.then_some(sel);
            egui::Area::new(popup.with("area"))
                .order(egui::Order::Foreground)
                .fixed_pos(anchor)
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
                            for (i, field) in fields.iter().take(12).enumerate() {
                                let (rect, row) =
                                    ui.allocate_exact_size(vec2(360.0, 24.0), Sense::click());
                                if i == sel || row.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(5),
                                        if i == sel { pal.selection } else { pal.hover },
                                    );
                                }
                                let name_color = if field.deprecated {
                                    pal.faint
                                } else {
                                    pal.syntax_key
                                };
                                ui.painter().text(
                                    pos2(rect.left() + 8.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &field.name,
                                    mono(12.5),
                                    name_color,
                                );
                                ui.painter().text(
                                    pos2(rect.right() - 8.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    &field.type_ref,
                                    mono(11.5),
                                    pal.info,
                                );
                                if row.clicked() {
                                    chosen = Some(i);
                                }
                            }
                        });
                });
            ui.data_mut(|d| d.insert_temp(sel_id, sel));
            if let Some(i) = chosen {
                let field: &Field = fields[i];
                let insert = completion_text(field, schema);
                let start_char = text[..start].chars().count();
                text.replace_range(start..byte, &insert.0);
                let cursor = start_char + insert.1;
                let mut state = output.state.clone();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(cursor))));
                state.store(ui.ctx(), id);
                changed = true;
                still_open = false;
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(popup.with("open"), still_open));
    changed
}

/// Text inserted for a field and the caret offset (chars) within it: object
/// fields get a selection set, fields with required arguments get `()`.
fn completion_text(field: &Field, schema: &Schema) -> (String, usize) {
    let required: Vec<&str> = field
        .args
        .iter()
        .filter(|a| a.type_ref.ends_with('!'))
        .map(|a| a.name.as_str())
        .collect();
    let mut out = field.name.clone();
    let mut caret = None;
    if !required.is_empty() {
        out.push('(');
        caret = Some(out.chars().count() + required[0].len() + 2);
        out.push_str(
            &required
                .iter()
                .map(|a| format!("{a}: "))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push(')');
    }
    let is_object = schema
        .types
        .get(&field.named)
        .is_some_and(|t| !t.fields.is_empty() && t.kind != "INPUT_OBJECT");
    if is_object {
        out.push_str(" {  }");
        if caret.is_none() {
            caret = Some(out.chars().count() - 2);
        }
    }
    let caret = caret.unwrap_or(out.chars().count());
    (out, caret)
}

/// Collapsible schema explorer of root fields and their types.
pub fn explorer(ui: &mut Ui, schema: &Schema) {
    let pal = Palette::of(ui);
    egui::ScrollArea::vertical()
        .id_salt("gql-explorer")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (label, root) in [
                ("Query", &schema.query_type),
                ("Mutation", &schema.mutation_type),
                ("Subscription", &schema.subscription_type),
            ] {
                let Some(root) = root else { continue };
                let Some(def) = schema.types.get(root) else {
                    continue;
                };
                widgets::caption(ui, &label.to_uppercase());
                for field in &def.fields {
                    field_row(ui, schema, field, &pal, 0);
                }
                ui.add_space(8.0);
            }
        });
}

fn field_row(ui: &mut Ui, schema: &Schema, field: &Field, pal: &Palette, depth: usize) {
    let child_type = schema
        .types
        .get(&field.named)
        .filter(|t| !t.fields.is_empty());
    let header = RichText::new(format!("{}: {}", field.name, field.type_ref))
        .font(mono(12.0))
        .color(if field.deprecated {
            pal.faint
        } else {
            pal.text
        });
    match child_type {
        Some(t) if depth < 4 => {
            egui::CollapsingHeader::new(header)
                .id_salt(ui.id().with((&field.name, depth, &t.name)))
                .show(ui, |ui| {
                    if !field.description.is_empty() {
                        ui.label(RichText::new(&field.description).size(11.5).color(pal.weak));
                    }
                    for f in &t.fields {
                        field_row(ui, schema, f, pal, depth + 1);
                    }
                });
        }
        _ => {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(header).on_hover_text(&field.description);
            });
        }
    }
}

/// The GraphQL body: schema status, query editor with completion, variables
/// and the explorer. Returns whether the request changed.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &mut Ui,
    query: &mut String,
    variables: &mut String,
    vars: &Map<String, Value>,
    schema: Option<&Schema>,
    loading: bool,
    error: Option<&str>,
    fetch: &mut bool,
) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    let show_explorer_id = ui.id().with("gql-show-explorer");
    let mut show_explorer: bool = ui.data(|d| d.get_temp(show_explorer_id).unwrap_or(true));
    ui.horizontal(|ui| match (schema, loading) {
        (_, true) => {
            ui.add(egui::Spinner::new().size(12.0));
            ui.label(RichText::new("Fetching schema…").size(12.0).color(pal.weak));
        }
        (Some(s), _) => {
            widgets::pill(
                ui,
                &format!("Schema · {} types", s.visible_types().count()),
                pal.success,
                false,
            );
            if ui.small_button("Refresh").clicked() {
                *fetch = true;
            }
            ui.checkbox(&mut show_explorer, "Explorer");
        }
        (None, _) => {
            if widgets::secondary_button(ui, "Fetch schema")
                .on_hover_text("Run introspection against this URL")
                .clicked()
            {
                *fetch = true;
            }
            if let Some(e) = error {
                ui.label(RichText::new(e).size(12.0).color(pal.error));
            } else {
                ui.label(
                    RichText::new("Fetch the schema for completion (Ctrl+Space) and the explorer.")
                        .size(12.0)
                        .color(pal.weak),
                );
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(show_explorer_id, show_explorer));
    ui.add_space(6.0);
    let explorer_w = if schema.is_some() && show_explorer {
        250.0
    } else {
        0.0
    };
    let full = ui.available_rect_before_wrap();
    let editor_rect = egui::Rect::from_min_max(
        full.min,
        pos2(
            full.right() - explorer_w - if explorer_w > 0.0 { 8.0 } else { 0.0 },
            full.bottom(),
        ),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(editor_rect), |ui| {
        let h = ui.available_height();
        widgets::caption(ui, "Query");
        ui.allocate_ui(vec2(ui.available_width(), h * 0.62), |ui| {
            widgets::field_frame(&pal)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_min_height(ui.available_height() - 4.0);
                    changed |= query_editor(ui, query, schema, ui.id().with("gql-query"));
                });
        });
        ui.add_space(6.0);
        widgets::caption(ui, "Variables (JSON)");
        widgets::field_frame(&pal)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height() - 16.0);
                changed |= widgets::code_editor(
                    ui,
                    variables,
                    Syntax::Json,
                    vars,
                    ui.id().with("gql-vars"),
                )
                .changed();
            });
    });
    if let (Some(schema), true) = (schema, explorer_w > 0.0) {
        let rect = egui::Rect::from_min_max(pos2(full.right() - explorer_w, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            egui::Frame::new()
                .stroke(pal.hairline())
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_min_size(rect.size() - vec2(18.0, 18.0));
                    explorer(ui, schema);
                });
        });
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_text_adds_arguments_and_selection_sets() {
        let t = |kind: &str, name: &str| serde_json::json!({"kind": kind, "name": name, "ofType": null});
        let nn =
            |inner: Value| serde_json::json!({"kind": "NON_NULL", "name": null, "ofType": inner});
        let doc = serde_json::json!({"data": {"__schema": {"queryType": {"name": "Query"}, "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [
                {"name": "product", "args": [{"name": "sku", "type": nn(t("SCALAR", "String"))}], "type": t("OBJECT", "Product")},
                {"name": "me", "args": [], "type": t("OBJECT", "Product")}
            ]},
            {"kind": "OBJECT", "name": "Product", "fields": [{"name": "sku", "args": [], "type": t("SCALAR", "String")}]}
        ]}}});
        let schema = Schema::from_introspection(&doc).unwrap();
        let product = schema.types["Query"]
            .fields
            .iter()
            .find(|f| f.name == "product")
            .unwrap();
        assert_eq!(
            completion_text(product, &schema),
            ("product(sku: ) {  }".into(), 13)
        );
        let sku = &schema.types["Product"].fields[0];
        assert_eq!(completion_text(sku, &schema), ("sku".into(), 3));
        let me = schema.types["Query"]
            .fields
            .iter()
            .find(|f| f.name == "me")
            .unwrap();
        assert_eq!(completion_text(me, &schema), ("me {  }".into(), 5));
    }

    #[test]
    fn highlights_without_losing_text() {
        let pal = Palette::from_visuals(&egui::Visuals::dark());
        let q = "query Q($n: Int!) { products(first: $n) { sku # é\n name } }";
        assert_eq!(highlight(q, &pal).text, q);
    }
}
