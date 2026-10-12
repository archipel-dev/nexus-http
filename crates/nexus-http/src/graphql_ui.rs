//! The GraphQL body: query editor with schema completion, variables, and the schema
//! explorer. Built from kit components; the schema and its rules are
//! `nexus_http_core::graphql`.

use crate::views::template::Templates;
use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::code::GraphQl;
use archipel_kit::components::{
    Button, Checkbox, CodeEditor, Completer, Completion, Completions, FieldLabel, Spinner,
    SplitPane, Tag, Tooltip, Tree, TreeRow,
};
use archipel_kit::parts::Tone;
use archipel_kit::tokens::size;
use egui::Ui;
use nexus_http_core::graphql::{Field, Schema};
use std::collections::HashSet;

/// Share of the editor column the query takes; the variables get the rest.
const QUERY_SHARE: f32 = 0.62;
/// Default share of the body the editors take beside the explorer.
const EDITOR_RATIO: f32 = 0.7;
/// How deep the explorer unfolds nested types.
const EXPLORER_DEPTH: usize = 4;

/// Completes fields of the schema at the caret.
struct SchemaFields<'s>(&'s Schema);

impl SchemaFields<'_> {
    fn fields(&self, text: &str, caret: usize, need_prefix: bool) -> Option<Completions> {
        let byte = text
            .char_indices()
            .nth(caret)
            .map_or(text.len(), |(b, _)| b);
        let (start, fields) = self.0.complete(text, byte)?;
        if need_prefix && start == byte {
            return None;
        }
        Some(Completions {
            start: text[..start].chars().count(),
            end: caret,
            items: fields
                .into_iter()
                .map(|field| {
                    let (insert, at) = self.0.insertion(field);
                    Completion::new(field.name.clone(), insert)
                        .detail(field.type_ref.clone())
                        .caret(at)
                })
                .collect(),
        })
    }
}

impl Completer for SchemaFields<'_> {
    /// While typing: only once a field name is started.
    fn complete(&self, text: &str, caret: usize) -> Option<Completions> {
        self.fields(text, caret, true)
    }

    /// Ctrl+Space: every field of the type at the caret.
    fn complete_on_request(&self, text: &str, caret: usize) -> Option<Completions> {
        self.fields(text, caret, false)
    }
}

/// The GraphQL body: schema status, query editor with completion, variables and the
/// explorer. Returns whether the request changed.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &mut Ui,
    query: &mut String,
    variables: &mut String,
    templates: &Templates,
    schema: Option<&Schema>,
    loading: bool,
    error: Option<&str>,
    fetch: &mut bool,
) -> bool {
    let mut changed = false;
    let explorer_id = ui.id().with("gql-show-explorer");
    let mut show_explorer: bool = ui.data(|d| d.get_temp(explorer_id).unwrap_or(true));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        match (schema, loading) {
            (_, true) => {
                ui.add(Spinner::new());
                ui.add(FieldLabel::new("Fetching schema…").enabled(false));
            }
            (Some(s), _) => {
                ui.add(
                    Tag::new(format!("Schema · {} types", s.visible_types().count()))
                        .tone(Tone::Success),
                );
                *fetch |= ui.add(Button::ghost("Refresh")).clicked();
                ui.add(Checkbox::new(&mut show_explorer, "Explorer"));
            }
            (None, _) => {
                let button = Button::secondary("Fetch schema")
                    .with(Tooltip::new("Run introspection against this URL"));
                *fetch |= ui.add(button).clicked();
                match error {
                    Some(e) => crate::views::request::problem(ui, e),
                    None => {
                        ui.add(
                            FieldLabel::new(
                                "Fetch the schema for completion (Ctrl+Space) and the explorer.",
                            )
                            .enabled(false),
                        );
                    }
                }
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(explorer_id, show_explorer));

    let mut editors = |ui: &mut Ui| {
        let height = ui.available_height();
        let completer = schema.map(SchemaFields);
        let mut editor = CodeEditor::new(query)
            .label("Query")
            .highlighter(&GraphQl)
            .line_numbers(true)
            .height(height * QUERY_SHARE);
        if let Some(completer) = &completer {
            editor = editor.completer(completer);
        }
        changed |= ui.add(editor).changed();
        ui.add(FieldLabel::new("Variables (JSON)"));
        changed |= ui
            .add(
                CodeEditor::new(variables)
                    .label("Variables")
                    .highlighter(&templates.json)
                    .line_numbers(true)
                    .height(ui.available_height()),
            )
            .changed();
    };
    match schema.filter(|_| show_explorer) {
        Some(schema) => {
            let ratio_id = ui.id().with("gql-explorer-ratio");
            let mut ratio = ui.data(|d| d.get_temp(ratio_id).unwrap_or(EDITOR_RATIO));
            SplitPane::horizontal("gql-explorer", &mut ratio)
                .default_ratio(EDITOR_RATIO)
                .label("Resize schema explorer")
                .show(ui, editors, |ui| explorer(ui, schema));
            ui.data_mut(|d| d.insert_temp(ratio_id, ratio));
        }
        None => editors(ui),
    }
    changed
}

/// The schema explorer: root operations, their fields and nested types, unfolded on demand.
pub fn explorer(ui: &mut Ui, schema: &Schema) {
    let open_id = ui.id().with("gql-explorer-open");
    let mut open: HashSet<String> = ui.data(|d| d.get_temp(open_id)).unwrap_or_default();
    let mut rows = Vec::new();
    let mut paths = Vec::new();
    for (label, root) in [
        ("Query", &schema.query_type),
        ("Mutation", &schema.mutation_type),
        ("Subscription", &schema.subscription_type),
    ] {
        let Some(def) = root.as_ref().and_then(|r| schema.types.get(r)) else {
            continue;
        };
        let path = label.to_owned();
        let expanded = !open.contains(&format!("-{path}"));
        rows.push(TreeRow::new(label, 0).parent(expanded));
        paths.push(format!("-{path}"));
        if expanded {
            for field in &def.fields {
                field_rows(schema, field, &path, 1, &open, &mut rows, &mut paths);
            }
        }
    }
    let mut selected = None;
    let out = Tree::new("gql-explorer-tree", &rows, &mut selected)
        .name("Schema explorer")
        .height(ui.available_height())
        .show(ui);
    if let Some(index) = out.toggled
        && let Some(path) = paths.get(index)
        && !open.remove(path)
    {
        open.insert(path.clone());
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
}

/// `field` and, when unfolded, the fields of its type. Root sections are open unless their
/// `-path` is in `open`; fields are closed unless their path is.
fn field_rows(
    schema: &Schema,
    field: &Field,
    parent: &str,
    depth: usize,
    open: &HashSet<String>,
    rows: &mut Vec<TreeRow>,
    paths: &mut Vec<String>,
) {
    let path = format!("{parent}.{}", field.name);
    let children = schema
        .types
        .get(&field.named)
        .filter(|t| !t.fields.is_empty() && depth < EXPLORER_DEPTH);
    let mut row = TreeRow::new(field.name.clone(), depth).secondary(field.type_ref.clone());
    if field.deprecated {
        row = row.icon_tone(Tone::Muted);
    }
    let expanded = open.contains(&path);
    if children.is_some() {
        row = row.parent(expanded);
    }
    rows.push(row);
    paths.push(path.clone());
    if let (Some(t), true) = (children, expanded) {
        for child in &t.fields {
            field_rows(schema, child, &path, depth + 1, open, rows, paths);
        }
    }
}
