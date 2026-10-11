//! Modal overlays: environment editor, request switcher (Ctrl+P), import,
//! export and code generation.

use crate::state::NexusState;
use crate::theme::{Palette, mono, semibold};
use crate::widgets::{
    self, Icon, Syntax, caption, code_editor, code_view, field_frame, icon_button, primary_button,
    secondary_button,
};
use egui::{Align, CornerRadius, Layout, RichText, Sense, Ui, vec2};
use nexus_http_core::codegen::{self, Target};
use nexus_http_core::exec::{PrepareOptions, prepare};
use nexus_http_core::import::{self, ImportError};
use nexus_http_core::model::{Environment, Id};
use serde_json::{Map, Value};

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
    cursor: usize,
    // import / export
    import_text: String,
    import_path: String,
    import_merge: bool,
    import_result: Option<Result<String, String>>,
    export_path: String,
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
                self.cursor = 0;
            }
            Overlay::Environments => self.env_loaded_for = None,
            Overlay::Import => self.import_result = None,
            _ => {}
        }
    }
}

fn modal_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.raised)
        .stroke(pal.hairline())
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(16))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: egui::Color32::from_black_alpha(if pal.dark { 120 } else { 40 }),
        })
}

/// Renders the open overlay, if any.
pub fn show(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi) {
    let pal = Palette::of(ui);
    let ctx = ui.ctx().clone();
    let id = egui::Id::new("nexus-overlay");
    let close = match local.open {
        Overlay::None => return,
        Overlay::Switcher => {
            let area = egui::Area::new(id.with("switcher-area"))
                .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 80.0));
            egui::Modal::new(id.with("switcher"))
                .area(area)
                .frame(modal_frame(&pal).inner_margin(egui::Margin::same(8)))
                .show(&ctx, |ui| switcher(ui, state, local, &pal))
                .should_close()
        }
        Overlay::Environments => egui::Modal::new(id.with("env"))
            .frame(modal_frame(&pal))
            .show(&ctx, |ui| environments(ui, state, local, &pal))
            .should_close(),
        Overlay::Import => egui::Modal::new(id.with("import"))
            .frame(modal_frame(&pal))
            .show(&ctx, |ui| import_dialog(ui, state, local, &pal))
            .should_close(),
        Overlay::Export => egui::Modal::new(id.with("export"))
            .frame(modal_frame(&pal))
            .show(&ctx, |ui| export_dialog(ui, state, local, &pal))
            .should_close(),
        Overlay::Code => egui::Modal::new(id.with("code"))
            .frame(modal_frame(&pal))
            .show(&ctx, |ui| code_dialog(ui, state, local, &pal))
            .should_close(),
    };
    if close {
        local.open = Overlay::None;
    }
}

fn title_row(ui: &mut Ui, pal: &Palette, title: &str, local: &mut OverlayUi) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(semibold(ui, 15.0))
                .color(pal.strong),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, Icon::Close, pal.weak, "Close (Esc)").clicked() {
                local.open = Overlay::None;
            }
        });
    });
    ui.add_space(10.0);
}

// ------------------------------------------------------------------ switcher

fn switcher(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi, pal: &Palette) {
    ui.set_width(560.0);
    let matches = state.switcher_matches(&local.query);
    field_frame(pal).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
            widgets::paint_icon(ui.painter(), r, Icon::Search, pal.weak);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut local.query)
                    .frame(false)
                    .hint_text("Go to request…")
                    .desired_width(ui.available_width()),
            );
            if local.focus_pending {
                edit.request_focus();
                local.focus_pending = false;
            }
            if edit.changed() {
                local.cursor = 0;
            }
        });
    });
    let (down, up, enter) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    if down && local.cursor + 1 < matches.len() {
        local.cursor += 1;
    }
    if up {
        local.cursor = local.cursor.saturating_sub(1);
    }
    ui.add_space(6.0);
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            if matches.is_empty() {
                ui.label(RichText::new("No matching requests").color(pal.weak));
            }
            for (i, (path, id)) in matches.iter().enumerate() {
                let Some((_, request)) = state.workspace.find_request(id) else {
                    continue;
                };
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                let active = i == local.cursor;
                if active || resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(6),
                        if active { pal.selection } else { pal.hover },
                    );
                }
                if active {
                    resp.scroll_to_me(None);
                }
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(vec2(8.0, 0.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                );
                widgets::request_label(&mut child, request, 40.0);
                child.add_space(8.0);
                child.label(RichText::new(&request.name).color(pal.strong));
                if !path.is_empty() {
                    child.label(RichText::new(path).size(12.0).color(pal.weak));
                }
                child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&request.url)
                                .font(mono(11.5))
                                .color(pal.faint),
                        )
                        .truncate(),
                    );
                });
                if resp.clicked() {
                    chosen = Some(id.clone());
                }
            }
        });
    if enter {
        chosen = matches.get(local.cursor).map(|(_, id)| id.clone());
    }
    if let Some(id) = chosen {
        state.select(id);
        local.open = Overlay::None;
    }
}

// ------------------------------------------------------------------ environments

fn environments(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi, pal: &Palette) {
    ui.set_width(760.0);
    ui.set_height(480.0);
    title_row(ui, pal, "Manage Environments", local);
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
    ui.horizontal_top(|ui| {
        // Left list
        ui.allocate_ui_with_layout(
            vec2(200.0, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                let mut pick: Option<Option<Id>> = None;
                if env_row(
                    ui,
                    pal,
                    "Base Environment",
                    None,
                    key.is_none(),
                    state.workspace.active_environment.is_none(),
                )
                .clicked()
                {
                    pick = Some(None);
                }
                ui.add_space(6.0);
                caption(ui, "SUB ENVIRONMENTS");
                ui.add_space(2.0);
                for env in &state.workspace.environments {
                    let color = env.color.as_deref().and_then(widgets::parse_color);
                    let active = state.workspace.active_environment.as_ref() == Some(&env.id);
                    if env_row(
                        ui,
                        pal,
                        &env.name,
                        color,
                        key.as_ref() == Some(&env.id),
                        active,
                    )
                    .clicked()
                    {
                        pick = Some(Some(env.id.clone()));
                    }
                }
                ui.add_space(6.0);
                if ui.button("+ New environment").clicked() {
                    let env = Environment::new(format!(
                        "Environment {}",
                        state.workspace.environments.len() + 1
                    ));
                    let id = env.id.clone();
                    state.workspace.environments.push(env);
                    state.touch();
                    pick = Some(Some(id));
                }
                if let Some(p) = pick {
                    local.env_selected = p;
                }
            },
        );
        ui.separator();
        // Right editor
        ui.vertical(|ui| {
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
                        .add(
                            egui::TextEdit::singleline(&mut env.name)
                                .font(semibold(ui, 14.0))
                                .desired_width(220.0),
                        )
                        .changed();
                    for c in [
                        "#4E9D6C", "#3871E1", "#D59637", "#C54E58", "#9E6AD8", "#2AACB8",
                    ] {
                        let color = widgets::parse_color(c).unwrap_or(pal.accent);
                        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 6.5, color);
                        if env.color.as_deref() == Some(c) {
                            ui.painter().circle_stroke(
                                r.center(),
                                8.5,
                                egui::Stroke::new(1.5_f32, pal.strong),
                            );
                        }
                        if resp.clicked() {
                            env.color = Some(c.to_owned());
                            changed = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(RichText::new("Delete").color(pal.error))
                            .clicked()
                        {
                            delete = true;
                        }
                    });
                });
                if changed {
                    state.touch();
                }
            } else {
                ui.label(
                    RichText::new("Base Environment")
                        .font(semibold(ui, 14.0))
                        .color(pal.strong),
                );
                ui.label(
                    RichText::new("Variables here apply to every sub environment.")
                        .size(12.0)
                        .color(pal.weak),
                );
            }
            ui.add_space(8.0);
            let vars = Map::new();
            let h = ui.available_height() - 64.0;
            let edited = field_frame(pal)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_height(h);
                    code_editor(
                        ui,
                        &mut local.env_text,
                        Syntax::Json,
                        &vars,
                        ui.id().with("envjson"),
                    )
                    .changed()
                })
                .inner;
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
                    ui.label(
                        RichText::new(format!("Invalid JSON: {e}"))
                            .size(12.0)
                            .color(pal.warning),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("Use values as {{ name }} or {{ nested.key }} in any field.")
                            .size(12.0)
                            .color(pal.weak),
                    );
                }
            }
            if let Some(env) = env_mut(state, selected.as_ref()) {
                let mut secrets = env.secrets.join(", ");
                let resp = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new("Secret keys").size(12.0).color(pal.weak));
                        ui.add(
                            egui::TextEdit::singleline(&mut secrets)
                                .hint_text("token, password — kept out of git")
                                .desired_width(ui.available_width()),
                        )
                    })
                    .inner;
                if resp.changed() {
                    env.secrets = secrets
                        .split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                    state.touch();
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
        });
    });
}

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

fn env_row(
    ui: &mut Ui,
    pal: &Palette,
    name: &str,
    color: Option<egui::Color32>,
    selected: bool,
    active: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), pal.selection);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), pal.hover);
    }
    ui.painter().circle_filled(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        4.0,
        color.unwrap_or(pal.faint),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 24.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(13.0),
        pal.text,
    );
    if active {
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "active",
            egui::FontId::proportional(11.0),
            pal.success,
        );
    }
    resp
}

// ------------------------------------------------------------------ import / export

fn import_dialog(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi, pal: &Palette) {
    ui.set_width(680.0);
    title_row(ui, pal, "Import", local);
    ui.label(RichText::new("Paste a curl command, an Insomnia v4/v5 export, a Postman v2.1 collection or an OpenAPI 3 document (JSON or YAML) — or load it from a file.").color(pal.weak));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        field_frame(pal).show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut local.import_path)
                    .frame(false)
                    .hint_text("/path/to/collection.json")
                    .desired_width(520.0),
            );
        });
        if secondary_button(ui, "Load file").clicked() {
            match std::fs::read_to_string(local.import_path.trim()) {
                Ok(text) => local.import_text = text,
                Err(e) => {
                    local.import_result = Some(Err(format!(
                        "Cannot read {}: {e}",
                        local.import_path.trim()
                    )))
                }
            }
        }
    });
    ui.add_space(8.0);
    let vars = Map::new();
    field_frame(pal)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_height(260.0);
            code_editor(
                ui,
                &mut local.import_text,
                Syntax::Json,
                &vars,
                ui.id().with("import"),
            );
        });
    ui.add_space(8.0);
    if let Some(result) = &local.import_result {
        let (text, color) = match result {
            Ok(t) => (t.as_str(), pal.success),
            Err(e) => (e.as_str(), pal.error),
        };
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        ui.checkbox(&mut local.import_merge, "Add to the current workspace");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary_button(ui, "Import", !local.import_text.trim().is_empty()).clicked() {
                local.import_result =
                    Some(run_import(state, &local.import_text, local.import_merge));
                if matches!(local.import_result, Some(Ok(_))) {
                    local.import_text.clear();
                }
            }
            if secondary_button(ui, "Cancel").clicked() {
                local.open = Overlay::None;
            }
        });
    });
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

fn export_dialog(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi, pal: &Palette) {
    ui.set_width(560.0);
    title_row(ui, pal, "Export Workspace", local);
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
    ui.label(
        RichText::new("Writes a single-file nexus-http export. Secret values are left out.")
            .color(pal.weak),
    );
    ui.add_space(8.0);
    field_frame(pal).show(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut local.export_path)
                .frame(false)
                .desired_width(ui.available_width()),
        );
    });
    ui.add_space(10.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if primary_button(ui, "Export", true).clicked() {
            let text = import::export(&state.workspace);
            match std::fs::write(local.export_path.trim(), text) {
                Ok(()) => {
                    state.notify(format!("Exported to {}", local.export_path.trim()));
                    local.open = Overlay::None;
                }
                Err(e) => state.notify(format!("Export failed: {e}")),
            }
        }
        if secondary_button(ui, "Cancel").clicked() {
            local.open = Overlay::None;
        }
    });
}

// ------------------------------------------------------------------ code generation

fn code_dialog(ui: &mut Ui, state: &mut NexusState, local: &mut OverlayUi, pal: &Palette) {
    ui.set_width(820.0);
    ui.set_height(500.0);
    title_row(ui, pal, "Generate Code", local);
    let target = *local.target.get_or_insert(Target::Curl);
    let Some(id) = state.selected.clone() else {
        ui.label("Select a request first.");
        return;
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
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            vec2(190.0, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                for t in Target::ALL {
                    let (rect, resp) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                    if t == target {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(6), pal.selection);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(6), pal.hover);
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 10.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        t.label(),
                        egui::FontId::proportional(13.0),
                        pal.text,
                    );
                    if resp.clicked() {
                        local.target = Some(t);
                    }
                }
            },
        );
        ui.vertical(|ui| match &code {
            Ok(code) => {
                ui.horizontal(|ui| {
                    caption(
                        ui,
                        "Variables and template tags are resolved with the active environment.",
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if secondary_button(ui, "Copy").clicked() {
                            ui.ctx().copy_text(code.clone());
                            state.notify("Code copied to the clipboard");
                        }
                    });
                });
                ui.add_space(6.0);
                field_frame(pal)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_height(ui.available_height() - 6.0);
                        ui.set_width(ui.available_width());
                        code_view(ui, code, Syntax::Plain, "generated");
                    });
            }
            Err(e) => {
                ui.label(RichText::new(e).color(pal.error));
            }
        });
    });
}
