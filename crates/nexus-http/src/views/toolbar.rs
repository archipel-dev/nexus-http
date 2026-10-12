//! The main toolbar of the standalone window, laid out like Android Studio's: the workspace
//! widget (the project widget: its tile, name and menu), the environment widget (where
//! Android Studio has its version control widget) and, on the right, Run collection and Go to
//! request. A plugin host owns its own window, so [`crate::NexusApp::ui`] has none.

use crate::state::NexusState;
use archipel_kit::components::{
    Avatar, IconButton, MenuItem, MenuSeparator, ToolbarButton, WindowChrome,
};
use archipel_kit::parts::{Popover, Slot};
use archipel_kit::{Glyph, egui, tokens};
use egui::{Align, Layout, Ui};
use nexus_http_core::model::Id;

/// How the standalone window meets the native title bar: on macOS the toolbar takes the
/// title bar's place and holds the traffic lights.
pub const CHROME: WindowChrome = WindowChrome::Integrated;

/// Width of the workspace menu.
const WORKSPACE_MENU_WIDTH: f32 = 220.0;
/// Width of the environment menu.
const ENVIRONMENT_MENU_WIDTH: f32 = 220.0;
/// Name of the environment every workspace has.
pub const BASE_ENVIRONMENT: &str = "Base Environment";

/// Which toolbar menu is open.
#[derive(Debug, Default)]
pub struct ToolbarUi {
    pub workspace_menu: bool,
    pub environment_menu: bool,
}

/// What the toolbar asked the app to do.
#[derive(Debug, Default)]
pub struct ToolbarActions {
    pub open_import: bool,
    pub export: bool,
    /// Open the collection runner for the whole workspace.
    pub run_collection: bool,
    pub open_environments: bool,
    /// Open the request switcher (Ctrl+P).
    pub go_to_request: bool,
}

/// The color of the workspace's tile, which the toolbar's glow continues.
pub fn workspace_color(state: &NexusState) -> egui::Color32 {
    archipel_kit::theme::avatar_color(&state.workspace.name)
}

/// The workspace widget: tile with the initials, name and chevron.
pub fn workspace_button(name: &str, open: bool) -> ToolbarButton<'static> {
    let tile = name.to_owned();
    ToolbarButton::new(name)
        .leading(Slot::ui_width(tokens::toolbar::ICON, move |ui| {
            ui.add(Avatar::new(tile).size(tokens::toolbar::ICON));
        }))
        .dropdown(true)
        .open(open)
}

/// The name of the active environment.
pub fn environment_name(state: &NexusState) -> String {
    state
        .workspace
        .active_environment()
        .map_or(BASE_ENVIRONMENT.to_owned(), |e| e.name.clone())
}

/// The environment widget: the active environment and a chevron.
pub fn environment_button(name: &str, open: bool) -> ToolbarButton<'static> {
    ToolbarButton::new(name)
        .icon(Glyph::Globe)
        .dropdown(true)
        .open(open)
}

/// Opens the request switcher.
pub fn go_to_request_button() -> IconButton {
    IconButton::new(Glyph::Search, "Go to request").large(true)
}

/// Opens the collection runner for the whole workspace.
pub fn run_collection_button() -> IconButton {
    IconButton::new(Glyph::Run, "Run collection").large(true)
}

/// Draws the toolbar's content (the kit's `WindowToolbar` lays it out left to right).
pub fn show(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut ToolbarUi,
    actions: &mut ToolbarActions,
) {
    workspace_widget(ui, state, local, actions);
    environment_widget(ui, state, local, actions);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add(go_to_request_button()).clicked() {
            actions.go_to_request = true;
        }
        if ui.add(run_collection_button()).clicked() {
            actions.run_collection = true;
        }
    });
}

fn workspace_widget(
    ui: &mut Ui,
    state: &NexusState,
    local: &mut ToolbarUi,
    actions: &mut ToolbarActions,
) {
    let button = workspace_button(&state.workspace.name, local.workspace_menu).show(ui);
    if button.clicked() {
        local.workspace_menu = !local.workspace_menu;
    }
    let mut menu = local.workspace_menu;
    let closed = Popover::below("nexus-toolbar-workspace-menu", &button)
        .width(WORKSPACE_MENU_WIDTH)
        .show(ui.ctx(), &mut menu, |ui| {
            let mut chosen = false;
            if ui
                .add(MenuItem::new("Import…").icon(Glyph::Download))
                .clicked()
            {
                actions.open_import = true;
                chosen = true;
            }
            if ui
                .add(MenuItem::new("Export workspace…").icon(Glyph::Upload))
                .clicked()
            {
                actions.export = true;
                chosen = true;
            }
            ui.add(MenuSeparator);
            if ui
                .add(MenuItem::new("Run collection…").icon(Glyph::Run))
                .clicked()
            {
                actions.run_collection = true;
                chosen = true;
            }
            if ui
                .add(
                    MenuItem::new("Manage environments…")
                        .icon(Glyph::Settings)
                        .shortcut("Ctrl+E"),
                )
                .clicked()
            {
                actions.open_environments = true;
                chosen = true;
            }
            chosen
        });
    local.workspace_menu = menu && !closed.unwrap_or(false);
}

fn environment_widget(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut ToolbarUi,
    actions: &mut ToolbarActions,
) {
    let name = environment_name(state);
    let button = environment_button(&name, local.environment_menu).show(ui);
    if button.clicked() {
        local.environment_menu = !local.environment_menu;
    }
    let choices: Vec<(Option<Id>, String)> = std::iter::once((None, BASE_ENVIRONMENT.to_owned()))
        .chain(
            state
                .workspace
                .environments
                .iter()
                .map(|e| (Some(e.id.clone()), e.name.clone())),
        )
        .collect();
    let active = state.workspace.active_environment().map(|e| e.id.clone());
    let mut picked: Option<Option<Id>> = None;
    let mut menu = local.environment_menu;
    let closed = Popover::below("nexus-toolbar-environment-menu", &button)
        .width(ENVIRONMENT_MENU_WIDTH)
        .show(ui.ctx(), &mut menu, |ui| {
            let mut chosen = false;
            for (id, name) in &choices {
                if ui
                    .add(MenuItem::new(name.as_str()).checked(*id == active))
                    .clicked()
                {
                    picked = Some(id.clone());
                    chosen = true;
                }
            }
            ui.add(MenuSeparator);
            if ui
                .add(
                    MenuItem::new("Manage environments…")
                        .icon(Glyph::Settings)
                        .shortcut("Ctrl+E"),
                )
                .clicked()
            {
                actions.open_environments = true;
                chosen = true;
            }
            chosen
        });
    local.environment_menu = menu && !closed.unwrap_or(false);
    if let Some(id) = picked {
        state.set_active_environment(id);
    }
}
