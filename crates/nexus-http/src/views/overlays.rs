//! Modal overlays on archipel-kit dialogs: environment editor, request switcher (Ctrl+P),
//! import, export and code generation.

use crate::state::NexusState;
use archipel_kit::components::code::{Json, PlainText};
use archipel_kit::components::{
    Banner, Button, Checkbox, CodeEditor, CodeView, Dialog, DialogAction, Dropdown, EmptyState,
    Kbd, ListRow, Severity, Tag, TextField, VirtualList,
};
use archipel_kit::parts::Slot;
use archipel_kit::tokens::typography;
use archipel_kit::{Glyph, Theme, Tone, egui};
use egui::{Key, Modifiers, Ui, vec2};

/// Height of the environment editor.
const ENV_EDITOR_HEIGHT: f32 = 220.0;
/// Height of the import editor.
const IMPORT_EDITOR_HEIGHT: f32 = 240.0;
/// Height of the generated code.
const CODE_VIEW_HEIGHT: f32 = 330.0;
use nexus_http_core::codegen::{self, Target};
use nexus_http_core::exec::{PrepareOptions, prepare};
use nexus_http_core::import::{self, ImportError};
use nexus_http_core::model::{Environment, Id, Protocol};
use serde_json::Value;

#[derive(Default, PartialEq, Eq, Clone, Copy)]
pub enum Overlay {
    #[default]
    None,
    Environments,
    Switcher,
    Import,
    Export,
    Code,
}

/// Environment colors: (name shown in the dropdown, hex stored in the workspace).
const ENV_COLORS: [(&str, &str); 6] = [
    ("Green", "#4E9D6C"),
    ("Blue", "#3871E1"),
    ("Amber", "#D59637"),
    ("Red", "#C54E58"),
    ("Purple", "#9E6AD8"),
    ("Teal", "#2AACB8"),
];

#[derive(Default)]
pub struct OverlayUi {
    pub open: Overlay,
    // environments
    env_selected: Option<Id>,
    env_text: String,
    env_loaded_for: Option<Option<Id>>,
    env_error: Option<String>,
    // switcher
    query: String,
    cursor: Option<usize>,
    // import / export
    import_text: String,
    import_path: String,
    import_merge: bool,
    import_result: Option<Result<String, String>>,
    export_path: String,
    export_error: Option<String>,
    // codegen
    target: Option<Target>,
    focus_pending: bool,
}

impl OverlayUi {
    pub fn open(&mut self, overlay: Overlay) {
        self.open = overlay;
        self.focus_pending = true;
        match overlay {
            Overlay::Switcher => {
                self.query.clear();
                self.cursor = Some(0);
            }
            Overlay::Environments => self.env_loaded_for = None,
            Overlay::Import => self.import_result = None,
            Overlay::Export => self.export_error = None,
            _ => {}
        }
    }
}

/// Renders the open overlay, if any.
pub fn show(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi) {
    let ctx = ui.ctx().clone();
    let closed = match local.open {
        Overlay::None => return,
        Overlay::Switcher => switcher(&ctx, state, local),
        Overlay::Environments => environments(&ctx, state, local),
        Overlay::Import => import_dialog(&ctx, state, local),
        Overlay::Export => export_dialog(&ctx, state, local),
        Overlay::Code => code_dialog(&ctx, state, local),
    };
    if closed {
        local.open = Overlay::None;
    }
}

/// The method (or protocol) tag shown before a request's name.
fn method_tag(request: &nexus_http_core::model::Request) -> (String, Tone) {
    match request.protocol {
        Protocol::Http => {
            let method = request.method.to_string();
            let tone = Tone::method(&method);
            (method, tone)
        }
        Protocol::WebSocket => ("WS".into(), Tone::Accent),
        Protocol::Sse => ("SSE".into(), Tone::Link),
        Protocol::Grpc => ("gRPC".into(), Tone::Success),
    }
}

/// Keeps <kbd>Enter</kbd> typed into a multi-line editor from also confirming the dialog:
/// the kit dialog confirms on every Enter. Ctrl+Enter still confirms.
// TODO(kit Dialog): confirm only when no multi-line field has focus.
fn keep_enter_in_editor(ui: &Ui, editor: egui::Id) {
    if ui.memory(|m| m.has_focus(editor)) {
        ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    }
}

// ------------------------------------------------------------------ switcher

fn switcher(ctx: &egui::Context, state: &mut NexusState, local: &mut OverlayUi) -> bool {
    let matches = state.switcher_matches(&local.query);
    if matches.is_empty() {
        local.cursor = None;
    } else if local.cursor.is_none_or(|c| c >= matches.len()) {
        local.cursor = Some(0);
    }
    let focus = std::mem::take(&mut local.focus_pending);
    let mut chosen: Option<Id> = None;
    let cursor = &mut local.cursor;
    let query = &mut local.query;
    let action = Dialog::new("Go to request")
        .width(560.0)
        .content(|ui| {
            let field = ui.add(
                TextField::search(query)
                    .label("Request")
                    .placeholder("Go to request…"),
            );
            if focus {
                field.request_focus();
            }
            if field.changed() {
                *cursor = Some(0);
            }
            let (down, up) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                )
            });
            if down && let Some(c) = cursor.as_mut() {
                *c = (*c + 1).min(matches.len().saturating_sub(1));
            }
            if up && let Some(c) = cursor.as_mut() {
                *c = c.saturating_sub(1);
            }
            if matches.is_empty() {
                EmptyState::new("No matching requests")
                    .icon(Glyph::Search)
                    .show(ui);
                return;
            }
            let height = (matches.len() as f32 * 24.0).clamp(48.0, 336.0);
            let out = VirtualList::new("nexus-switcher", matches.len(), cursor)
                .name("Requests")
                .height(height)
                .show(ui, |i| {
                    let (path, id) = &matches[i];
                    let mut row = ListRow::new("").flat(true);
                    if let Some((_, request)) = state.workspace.find_request(id) {
                        let (method, tone) = method_tag(request);
                        row = ListRow::new(request.name.as_str())
                            .flat(true)
                            .secondary(path.as_str())
                            .trailing(Slot::ui_width(44.0, move |ui| {
                                ui.add(Tag::new(method).tone(tone));
                            }));
                    }
                    row
                });
            if let Some(i) = out.activated {
                chosen = matches.get(i).map(|(_, id)| id.clone());
            }
            ui.horizontal(|ui| {
                for (keys, what) in [("↑↓", "move"), ("Enter", "open"), ("Esc", "close")] {
                    ui.add(Kbd::new([keys]));
                    ui.label(what);
                }
            });
        })
        .primary("Open")
        .primary_enabled(!matches.is_empty())
        .secondary("Cancel")
        .show(ctx);
    match action {
        Some(DialogAction::Primary) => {
            chosen = chosen.or_else(|| {
                local
                    .cursor
                    .and_then(|c| matches.get(c))
                    .map(|(_, id)| id.clone())
            });
        }
        Some(_) => return true,
        None => {}
    }
    if let Some(id) = chosen {
        state.select(id);
        return true;
    }
    false
}

// ------------------------------------------------------------------ environments

fn env_ref<'a>(state: &'a NexusState, id: Option<&Id>) -> Option<&'a Environment> {
    match id {
        None => Some(&state.workspace.base_environment),
        Some(id) => state.workspace.environments.iter().find(|e| &e.id == id),
    }
}

fn env_mut<'a>(state: &'a mut NexusState, id: Option<&Id>) -> Option<&'a mut Environment> {
    match id {
        None => Some(&mut state.workspace.base_environment),
        Some(id) => state
            .workspace
            .environments
            .iter_mut()
            .find(|e| &e.id == id),
    }
}

fn environments(ctx: &egui::Context, state: &mut NexusState, local: &mut OverlayUi) -> bool {
    let key = local.env_selected.clone();
    if local.env_loaded_for.as_ref() != Some(&key) {
        let env = env_ref(state, key.as_ref());
        local.env_text = serde_json::to_string_pretty(&Value::Object(
            env.map(|e| e.data.clone()).unwrap_or_default(),
        ))
        .unwrap_or_default();
        local.env_error = None;
        local.env_loaded_for = Some(key.clone());
    }
    let editor_id = egui::Id::new("nexus-env-json");
    let action = Dialog::new("Manage Environments")
        .width(780.0)
        .content(|ui| {
            keep_enter_in_editor(ui, editor_id);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(200.0, 400.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| env_list(ui, state, local),
                );
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 400.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| env_editor(ui, state, local, editor_id),
                );
            });
        })
        .secondary("Close")
        .show(ctx);
    action.is_some()
}

fn env_list(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi) {
    let mut rows: Vec<(Option<Id>, String, bool)> = vec![(
        None,
        "Base Environment".into(),
        state.workspace.active_environment.is_none(),
    )];
    rows.extend(state.workspace.environments.iter().map(|e| {
        (
            Some(e.id.clone()),
            e.name.clone(),
            state.workspace.active_environment.as_ref() == Some(&e.id),
        )
    }));
    let mut selected = rows.iter().position(|(id, _, _)| *id == local.env_selected);
    let out = VirtualList::new("nexus-env-list", rows.len(), &mut selected)
        .name("Environments")
        .height(336.0)
        .show(ui, |i| {
            let (id, name, active) = &rows[i];
            let mut row = ListRow::new(name.as_str()).flat(true);
            row = row.icon(if id.is_none() {
                Glyph::Globe
            } else {
                Glyph::Server
            });
            if *active {
                row = row.trailing(Slot::ui_width(52.0, |ui| {
                    ui.add(Tag::new("active").tone(Tone::Success));
                }));
            }
            row
        });
    if out.selection_changed
        && let Some(i) = selected
    {
        local.env_selected = rows[i].0.clone();
    }
    ui.add_space(8.0);
    if ui
        .add(Button::secondary("New environment").icon(Glyph::Plus))
        .clicked()
    {
        let env = Environment::new(format!(
            "Environment {}",
            state.workspace.environments.len() + 1
        ));
        let id = env.id.clone();
        state.workspace.environments.push(env);
        state.touch();
        local.env_selected = Some(id);
    }
}

fn env_editor(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi, editor_id: egui::Id) {
    let selected = local.env_selected.clone();
    let mut delete = false;
    if let Some(env_id) = &selected {
        let Some(env) = state
            .workspace
            .environments
            .iter_mut()
            .find(|e| &e.id == env_id)
        else {
            local.env_selected = None;
            return;
        };
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui
                .add(TextField::new(&mut env.name).label("Name").width(280.0))
                .changed();
            let mut color = ENV_COLORS
                .iter()
                .find(|(_, hex)| env.color.as_deref() == Some(*hex))
                .map_or("Color", |(name, _)| *name);
            ui.add(
                Dropdown::new(
                    "nexus-env-color",
                    &mut color,
                    ENV_COLORS.iter().map(|(name, _)| *name),
                )
                .label("Color")
                .width(110.0),
            );
            if let Some((_, hex)) = ENV_COLORS.iter().find(|(name, _)| *name == color)
                && env.color.as_deref() != Some(*hex)
            {
                env.color = Some((*hex).to_owned());
                changed = true;
            }
            if ui.add(Button::danger("Delete")).clicked() {
                delete = true;
            }
        });
        if changed {
            state.touch();
        }
    } else {
        ui.label(
            egui::RichText::new("Base Environment")
                .font(typography::semibold(ui.ctx(), 14.0))
                .color(Theme::colors(ui.ctx()).text_strong),
        );
        ui.label(
            egui::RichText::new("Variables here apply to every sub environment.")
                .color(Theme::colors(ui.ctx()).text_secondary),
        );
    }
    if let Some(env) = env_mut(state, selected.as_ref()) {
        let mut secrets = env.secrets.join(", ");
        let resp = ui.add(
            TextField::new(&mut secrets)
                .label("Secret keys")
                .placeholder("token, password — kept out of git"),
        );
        if resp.changed() {
            env.secrets = secrets
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            state.touch();
        }
    }
    ui.add_space(8.0);
    let edited = ui
        .add(
            CodeEditor::new(&mut local.env_text)
                .id(editor_id)
                .label("Environment variables (JSON)")
                .highlighter(&Json)
                .line_numbers(true)
                .height(ENV_EDITOR_HEIGHT),
        )
        .changed();
    if edited {
        match serde_json::from_str::<Value>(&local.env_text) {
            Ok(Value::Object(map)) => {
                local.env_error = None;
                if let Some(env) = env_mut(state, selected.as_ref()) {
                    env.data = map;
                }
                state.touch();
            }
            Ok(_) => local.env_error = Some("The environment must be a JSON object".into()),
            Err(e) => local.env_error = Some(e.to_string()),
        }
    }
    ui.add_space(4.0);
    match &local.env_error {
        Some(e) => {
            Banner::new(Severity::Error, format!("Invalid JSON: {e}")).show(ui);
        }
        None => {
            ui.label(
                egui::RichText::new("Use values as {{ name }} or {{ nested.key }} in any field.")
                    .color(Theme::colors(ui.ctx()).text_secondary),
            );
        }
    }
    if delete && let Some(env_id) = &selected {
        state.workspace.environments.retain(|e| &e.id != env_id);
        if state.workspace.active_environment.as_ref() == Some(env_id) {
            state.workspace.active_environment = None;
        }
        local.env_selected = None;
        state.touch();
    }
}

// ------------------------------------------------------------------ import / export

fn import_dialog(ctx: &egui::Context, state: &mut NexusState, local: &mut OverlayUi) -> bool {
    let editor_id = egui::Id::new("nexus-import-text");
    let importable = !local.import_text.trim().is_empty();
    let focus = std::mem::take(&mut local.focus_pending);
    let action = Dialog::new("Import")
        .width(700.0)
        .message(
            "Paste a curl command, an Insomnia v4/v5 export, a Postman v2.1 collection or an \
             OpenAPI 3 document (JSON or YAML), or load it from a file. Ctrl+Enter imports.",
        )
        .content(|ui| {
            keep_enter_in_editor(ui, editor_id);
            ui.horizontal(|ui| {
                let width = ui.available_width() - 100.0;
                ui.add(
                    TextField::new(&mut local.import_path)
                        .label("File")
                        .placeholder("/path/to/collection.json")
                        .monospace(true)
                        .width(width),
                );
                if ui.add(Button::secondary("Load file")).clicked() {
                    match NexusState::read_import_file(&local.import_path) {
                        Ok(text) => local.import_text = text,
                        Err(e) => local.import_result = Some(Err(e)),
                    }
                }
            });
            ui.add(Checkbox::new(
                &mut local.import_merge,
                "Add to the current workspace",
            ));
            if focus {
                ui.memory_mut(|m| m.request_focus(editor_id));
            }
            ui.add(
                CodeEditor::new(&mut local.import_text)
                    .id(editor_id)
                    .label("Collection to import")
                    .placeholder("Paste a Postman, Insomnia or OpenAPI document, or a curl command")
                    .highlighter(&Json)
                    .line_numbers(true)
                    .height(IMPORT_EDITOR_HEIGHT),
            );
            match &local.import_result {
                Some(Ok(text)) => {
                    Banner::new(Severity::Success, text.as_str()).show(ui);
                }
                Some(Err(text)) => {
                    Banner::new(Severity::Error, text.as_str()).show(ui);
                }
                None => {}
            }
        })
        .primary("Import")
        .primary_enabled(importable)
        .secondary("Close")
        .show(ctx);
    match action {
        Some(DialogAction::Primary) => {
            local.import_result = Some(run_import(state, &local.import_text, local.import_merge));
            if matches!(local.import_result, Some(Ok(_))) {
                local.import_text.clear();
            }
            false
        }
        Some(_) => true,
        None => false,
    }
}

fn run_import(state: &mut NexusState, text: &str, merge: bool) -> Result<String, String> {
    let imported = import::import(text).map_err(|e: ImportError| e.to_string())?;
    let count = imported.workspace.requests().len();
    let mut message = format!(
        "Imported {count} request{} from {}.",
        if count == 1 { "" } else { "s" },
        imported.format.label()
    );
    for w in &imported.warnings {
        message.push_str(&format!("\n• {w}"));
    }
    if merge || imported.format == import::Format::Curl {
        state.merge_import(imported.workspace);
    } else {
        state.replace_workspace(imported.workspace);
    }
    Ok(message)
}

fn export_dialog(ctx: &egui::Context, state: &mut NexusState, local: &mut OverlayUi) -> bool {
    if local.export_path.is_empty() {
        let slug: String = state
            .workspace
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        local.export_path = state
            .base_dir()
            .join(format!("{slug}.nexus.json"))
            .display()
            .to_string();
    }
    let focus = std::mem::take(&mut local.focus_pending);
    let exportable = !local.export_path.trim().is_empty();
    let action = Dialog::new("Export Workspace")
        .width(560.0)
        .message("Writes a single-file nexus-http export. Secret values are left out.")
        .content(|ui| {
            let field = ui.add(
                TextField::new(&mut local.export_path)
                    .label("File")
                    .monospace(true),
            );
            if focus {
                field.request_focus();
            }
            if let Some(error) = &local.export_error {
                Banner::new(Severity::Error, error.as_str()).show(ui);
            }
        })
        .primary("Export")
        .primary_enabled(exportable)
        .secondary("Cancel")
        .show(ctx);
    match action {
        Some(DialogAction::Primary) => match state.export_workspace_to(&local.export_path) {
            Ok(()) => true,
            Err(e) => {
                local.export_error = Some(e);
                false
            }
        },
        Some(_) => true,
        None => false,
    }
}

// ------------------------------------------------------------------ code generation

fn code_dialog(ctx: &egui::Context, state: &mut NexusState, local: &mut OverlayUi) -> bool {
    let target = *local.target.get_or_insert(Target::Curl);
    let Some(id) = state.selected.clone() else {
        let action = Dialog::new("Generate Code")
            .message("Select a request first.")
            .secondary("Close")
            .show(ctx);
        return action.is_some();
    };
    let code = prepare(
        &state.workspace,
        &id,
        &PrepareOptions {
            base_dir: state.base_dir(),
            responses: &state.history,
        },
    )
    .map(|p| codegen::generate(&p, target))
    .map_err(|e| e.to_string());
    let mut picked = Target::ALL.iter().position(|t| *t == target);
    let action = Dialog::new("Generate Code")
        .width(840.0)
        .content(|ui| {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(190.0, 360.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        VirtualList::new("nexus-code-targets", Target::ALL.len(), &mut picked)
                            .name("Languages")
                            .height(360.0)
                            .show(ui, |i| ListRow::new(Target::ALL[i].label()).flat(true));
                    },
                );
                ui.vertical(|ui| match &code {
                    Ok(code) => {
                        ui.label(
                            egui::RichText::new(
                                "Variables and template tags are resolved with the active \
                                 environment.",
                            )
                            .color(Theme::colors(ui.ctx()).text_secondary),
                        );
                        ui.add_space(6.0);
                        CodeView::new(code)
                            .label("Generated code")
                            .highlighter(&PlainText)
                            .line_numbers(true)
                            .height(CODE_VIEW_HEIGHT)
                            .show(ui);
                    }
                    Err(e) => {
                        Banner::new(Severity::Error, e.as_str()).show(ui);
                    }
                });
            });
        })
        .primary("Copy")
        .primary_enabled(code.is_ok())
        .secondary("Close")
        .show(ctx);
    if let Some(i) = picked {
        local.target = Some(Target::ALL[i]);
    }
    match action {
        Some(DialogAction::Primary) => {
            if let Ok(code) = code {
                ctx.copy_text(code);
                state.notify("Code copied to the clipboard");
            }
            false
        }
        Some(_) => true,
        None => false,
    }
}
