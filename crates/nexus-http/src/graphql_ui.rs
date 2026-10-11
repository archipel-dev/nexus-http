//! GraphQL body: schema status, query and variables editors (kit `CodeEditor`) and the
//! schema explorer (kit `Tree`).

use crate::complete::Variables;
use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::code::{GraphQl, Json, TemplateHighlighter};
use archipel_kit::components::{
    Button, Checkbox, CodeEditor, SectionHeader, Spinner, Tag, Tooltip, Tree, TreeRow,
};
use archipel_kit::{Theme, Tone, egui};
use egui::{Id, Ui, pos2, vec2};
use nexus_http_core::graphql::{Field, Schema};
use std::collections::HashSet;

/// Deepest level of the explorer; recursive types would otherwise never end.
const MAX_DEPTH: usize = 4;

/// One visible explorer row and the path that identifies it.
struct Row {
    row: TreeRow,
    path: String,
}

/// Flattens the expanded part of the schema's root fields into tree rows.
fn flatten(schema: &Schema, expanded: &HashSet<String>) -> Vec<Row> {
    fn field(
        schema: &Schema,
        f: &Field,
        depth: usize,
        parent: &str,
        expanded: &HashSet<String>,
        out: &mut Vec<Row>,
    ) {
        let path = format!("{parent}/{}", f.name);
        let label = format!("{}: {}", f.name, f.type_ref);
        let children = schema
            .types
            .get(&f.named)
            .filter(|t| !t.fields.is_empty() && depth < MAX_DEPTH);
        let mut row = TreeRow::new(label, depth);
        if let Some(t) = children {
            let open = expanded.contains(&path);
            row = row.parent(open);
            out.push(Row {
                row,
                path: path.clone(),
            });
            if open {
                for child in &t.fields {
                    field(schema, child, depth + 1, &path, expanded, out);
                }
            }
        } else {
            out.push(Row { row, path });
        }
    }
    let mut out = Vec::new();
    for (label, root) in [
        ("Query", &schema.query_type),
        ("Mutation", &schema.mutation_type),
        ("Subscription", &schema.subscription_type),
    ] {
        let Some(def) = root.as_ref().and_then(|r| schema.types.get(r)) else {
            continue;
        };
        let path = label.to_owned();
        let open = !expanded.contains(&format!("!{path}"));
        out.push(Row {
            row: TreeRow::new(label, 0).parent(open),
            path: path.clone(),
        });
        if open {
            for f in &def.fields {
                field(schema, f, 1, &path, expanded, &mut out);
            }
        }
    }
    out
}

/// Collapsible schema explorer of root fields and their types.
pub fn explorer(ui: &mut Ui, schema: &Schema) {
    let key = Id::new("gql-explorer-open");
    let mut expanded: HashSet<String> = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let rows = flatten(schema, &expanded);
    let tree: Vec<TreeRow> = rows.iter().map(|r| r.row.clone()).collect();
    let mut selected = None;
    let out = Tree::new("gql-explorer", &tree, &mut selected)
        .height(ui.available_height())
        .show(ui);
    if let Some(i) = out.toggled {
        let path = &rows[i].path;
        // Root sections start open, so they record their *collapsed* state.
        let flag = if rows[i].row_depth_is_root() {
            format!("!{path}")
        } else {
            path.clone()
        };
        if !expanded.remove(&flag) {
            expanded.insert(flag);
        }
    }
    ui.data_mut(|d| d.insert_temp(key, expanded));
}

impl Row {
    fn row_depth_is_root(&self) -> bool {
        !self.path.contains('/')
    }
}

/// The GraphQL body: schema status, query editor, variables and the explorer.
/// Returns whether the request changed.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &mut Ui,
    query: &mut String,
    variables: &mut String,
    vars: &Variables,
    schema: Option<&Schema>,
    loading: bool,
    error: Option<&str>,
    fetch: &mut bool,
) -> bool {
    let colors = Theme::colors(ui.ctx());
    let mut changed = false;
    let show_explorer_id = ui.id().with("gql-show-explorer");
    let mut show_explorer: bool = ui.data(|d| d.get_temp(show_explorer_id)).unwrap_or(true);
    ui.horizontal(|ui| match (schema, loading) {
        (_, true) => {
            ui.add(Spinner::new());
            ui.label("Fetching schema…");
        }
        (Some(s), _) => {
            ui.add(
                Tag::new(format!("Schema · {} types", s.visible_types().count()))
                    .tone(Tone::Success),
            );
            if ui.add(Button::ghost("Refresh")).clicked() {
                *fetch = true;
            }
            ui.add(Checkbox::new(&mut show_explorer, "Explorer"));
        }
        (None, _) => {
            if ui
                .add(
                    Button::secondary("Fetch schema")
                        .with(Tooltip::new("Run introspection against this URL")),
                )
                .clicked()
            {
                *fetch = true;
            }
            match error {
                Some(e) => {
                    ui.label(egui::RichText::new(e).color(colors.error));
                }
                None => {
                    ui.label(
                        egui::RichText::new("Fetch the schema to browse it in the explorer.")
                            .color(colors.text_secondary),
                    );
                }
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
    let gap = if explorer_w > 0.0 { 8.0 } else { 0.0 };
    let editor_rect = egui::Rect::from_min_max(
        full.min,
        pos2(full.right() - explorer_w - gap, full.bottom()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(editor_rect), |ui| {
        let h = ui.available_height();
        let highlighter = TemplateHighlighter::new(|n: &str| vars.token(n)).inner(GraphQl);
        ui.add(SectionHeader::new("Query"));
        changed |= ui
            .add(
                CodeEditor::new(query)
                    .label("GraphQL query")
                    .highlighter(&highlighter)
                    .line_numbers(true)
                    .id(ui.id().with("gql-query"))
                    .height((h * 0.55).max(100.0)),
            )
            .changed();
        let json = TemplateHighlighter::new(|n: &str| vars.token(n)).inner(Json);
        ui.add(SectionHeader::new("Variables (JSON)"));
        changed |= ui
            .add(
                CodeEditor::new(variables)
                    .label("GraphQL variables")
                    .highlighter(&json)
                    .line_numbers(true)
                    .id(ui.id().with("gql-vars"))
                    .height((ui.available_height() - 8.0).max(80.0)),
            )
            .changed();
    });
    if let (Some(schema), true) = (schema, explorer_w > 0.0) {
        let rect = egui::Rect::from_min_max(pos2(full.right() - explorer_w, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.add(SectionHeader::new("Explorer"));
            ui.set_min_size(vec2(rect.width(), rect.height() - 24.0));
            explorer(ui, schema);
        });
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_rows_follow_the_expanded_set() {
        let t = |kind: &str, name: &str| serde_json::json!({"kind": kind, "name": name, "ofType": null});
        let doc = serde_json::json!({"data": {"__schema": {"queryType": {"name": "Query"}, "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [
                {"name": "me", "args": [], "type": t("OBJECT", "User")}
            ]},
            {"kind": "OBJECT", "name": "User", "fields": [{"name": "id", "args": [], "type": t("SCALAR", "ID")}]}
        ]}}});
        let schema = Schema::from_introspection(&doc).unwrap();
        let mut open = HashSet::new();
        assert_eq!(flatten(&schema, &open).len(), 2); // Query, me
        open.insert("Query/me".to_owned());
        assert_eq!(flatten(&schema, &open).len(), 3); // + id
        open.insert("!Query".to_owned());
        assert_eq!(flatten(&schema, &open).len(), 1);
    }
}
