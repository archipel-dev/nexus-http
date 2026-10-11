//! nexus-http: the views of Nexus, the API client of the Archipel suite.
//!
//! [`NexusApp`] renders the whole client into any `egui::Ui` — a plugin tool
//! window, an editor tab or the standalone app window — using the host's theme
//! colors. The engine lives in `nexus-http-core`; [`state::NexusState`] is the
//! UI-independent controller.

pub mod complete;
pub mod graphql_ui;
pub mod oauth;
pub mod plugin;
pub mod state;
pub mod theme;
pub mod views;
pub mod widgets;

pub use plugin::NexusHttpPlugin;

use archipel_kit::components::{EmptyState, Kbd, Severity, SplitPane, Toast};
use archipel_kit::parts::Surface;
use archipel_kit::tokens::size;
use archipel_kit::{Glyph, Theme};
use egui::{Key, KeyboardShortcut, Modifiers, Ui};
use nexus_http_core::model::{Auth, Item};
use nexus_http_core::template::Context;
use state::{NexusState, UiSettings};
use std::path::Path;
use views::overlays::{Overlay, OverlayUi};

/// Smallest widths (points) that keep every column usable at 1024x700.
const MIN_SIDEBAR: f32 = 200.0;
const MIN_EDITOR: f32 = 560.0;
const MIN_PANE: f32 = 280.0;
/// Narrowest pane the views are drawn in.
const MIN_CONTENT: f32 = 100.0;

pub struct NexusApp {
    pub state: NexusState,
    sidebar: views::sidebar::SidebarUi,
    response: views::response::ResponseUi,
    overlays: OverlayUi,
    runner: views::runner::RunnerUi,
    /// The collection runner replaces the request/response islands.
    pub runner_open: bool,
    layout_dirty: Option<std::time::Instant>,
    token_job: Option<(nexus_http_core::model::Id, oauth::TokenJob)>,
}

impl NexusApp {
    /// Installs the Archipel kit theme (fonts, spacing, colors) into `ctx`.
    /// Call once before the first frame; standalone hosts only, since a
    /// plugin host has already installed its theme.
    pub fn install(ctx: &egui::Context, dark: bool) {
        let theme = if dark {
            archipel_kit::Theme::dark()
        } else {
            archipel_kit::Theme::light()
        };
        archipel_kit::install(ctx, theme);
    }

    /// Opens (or creates) the workspace stored under `dir/.nexus`.
    pub fn open(dir: &Path) -> Self {
        Self::with_state(NexusState::open(dir))
    }

    pub fn with_state(state: NexusState) -> Self {
        Self {
            state,
            sidebar: Default::default(),
            response: Default::default(),
            overlays: Default::default(),
            runner: Default::default(),
            runner_open: false,
            layout_dirty: None,
            token_job: None,
        }
    }

    /// Shows the collection runner scoped to a folder (or the whole workspace).
    pub fn open_runner(&mut self, scope: Option<nexus_http_core::model::Id>) {
        self.runner.set_scope(scope);
        self.runner_open = true;
    }

    /// Starts the collection runner with its current options.
    pub fn start_run(&mut self) {
        self.runner_open = true;
        self.runner.start(&self.state);
    }

    pub fn open_overlay(&mut self, overlay: Overlay) {
        self.overlays.open(overlay);
    }

    /// Sends the selected request; for streaming requests connects, or sends
    /// the composed message when already connected.
    pub fn send_selected(&mut self) {
        let Some(id) = self.state.selected.clone() else {
            return;
        };
        let protocol = self
            .state
            .selected_request()
            .map(|r| r.protocol)
            .unwrap_or_default();
        match protocol {
            nexus_http_core::model::Protocol::Http => self.state.send(&id),
            _ if self.state.stream_connected(&id) => {
                if protocol == nexus_http_core::model::Protocol::WebSocket
                    || self.state.stream_can_send(&id)
                {
                    self.state.send_stream_message(&id);
                }
            }
            _ => self.state.connect_stream(&id),
        }
    }

    /// Background work; returns true while the UI should keep repainting.
    pub fn poll(&mut self) -> bool {
        if let Some((id, job)) = &self.token_job
            && let Some(result) = job.poll()
        {
            let id = id.clone();
            self.token_job = None;
            match result {
                Ok(token) => {
                    if let Some(Item::Request(r)) = self.state.workspace.find_item_mut(&id)
                        && let Auth::OAuth2(cfg) = &mut r.auth
                    {
                        cfg.token = Some(token);
                    } else if let Some(Item::Folder(f)) = self.state.workspace.find_item_mut(&id)
                        && let Auth::OAuth2(cfg) = &mut f.auth
                    {
                        cfg.token = Some(token);
                    }
                    self.state.touch();
                    self.state.notify("OAuth 2.0 token received");
                }
                Err(e) => self.state.notify(format!("Token request failed: {e}")),
            }
        }
        let running = self.runner.poll(&mut self.state);
        self.state.poll() || self.token_job.is_some() || running
    }

    fn start_token_fetch(&mut self) {
        let Some(id) = self.state.selected.clone() else {
            return;
        };
        let Some((folders, request)) = self.state.workspace.find_request(&id) else {
            return;
        };
        let Auth::OAuth2(cfg) = &request.auth else {
            return;
        };
        let ctx = Context::for_request(&self.state.workspace, &folders)
            .with_responses(&self.state.history);
        let render = |s: &str| ctx.render(s).map_err(|e| e.to_string());
        let rendered = (|| -> Result<_, String> {
            let mut c = (**cfg).clone();
            c.authorization_url = render(&c.authorization_url)?;
            c.access_token_url = render(&c.access_token_url)?;
            c.client_id = render(&c.client_id)?;
            c.client_secret = render(&c.client_secret)?;
            c.redirect_url = render(&c.redirect_url)?;
            c.scope = render(&c.scope)?;
            c.audience = render(&c.audience)?;
            c.username = render(&c.username)?;
            c.password = render(&c.password)?;
            Ok(c)
        })();
        match rendered {
            Ok(c) => {
                let job = oauth::TokenJob::start(c, self.state.workspace.settings.clone());
                self.token_job = Some((id, job));
                self.state.notify("Fetching OAuth 2.0 token…");
            }
            Err(e) => self.state.notify(format!("Cannot fetch token: {e}")),
        }
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        let cmd = Modifiers::COMMAND;
        let pressed = |ui: &mut Ui, key: Key| {
            ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(cmd, key)))
        };
        if self.overlays.open != Overlay::None {
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.overlays.open = Overlay::None;
            }
            return;
        }
        if pressed(ui, Key::Enter) {
            self.send_selected();
        }
        if pressed(ui, Key::L) {
            ui.memory_mut(|m| m.request_focus(egui::Id::new(views::request::URL_FIELD_ID)));
        }
        if pressed(ui, Key::P) {
            self.overlays.open(Overlay::Switcher);
        }
        if pressed(ui, Key::E) {
            self.overlays.open(Overlay::Environments);
        }
        if pressed(ui, Key::N) {
            let parent = self.state.selected.as_ref().and_then(|id| {
                self.state
                    .workspace
                    .find_request(id)
                    .and_then(|(f, _)| f.last().map(|f| f.id.clone()))
            });
            self.state.new_request(parent.as_ref());
            ui.memory_mut(|m| m.request_focus(egui::Id::new(views::request::URL_FIELD_ID)));
        }
        if pressed(ui, Key::S) {
            self.state.flush();
            self.state.notify("Saved");
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) && self.state.is_sending() {
            self.state.cancel();
        }
    }

    /// Renders the client into `ui`, filling the available rect.
    pub fn ui(&mut self, ui: &mut Ui) {
        if self.poll() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.shortcuts(ui);
        let secrets: std::collections::HashSet<String> =
            std::iter::once(&self.state.workspace.base_environment)
                .chain(&self.state.workspace.environments)
                .flat_map(|e| e.secrets.iter().cloned())
                .collect();
        ui.data_mut(|d| {
            d.insert_temp(
                egui::Id::new(state::SECRET_KEYS_ID),
                std::sync::Arc::new(secrets),
            )
        });
        let colors = Theme::colors(ui.ctx());
        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, colors.frame);
        let area = full.shrink(size::ISLAND_GAP);
        let mut shell = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );

        let mut side_actions = views::sidebar::SidebarActions::default();
        let mut req_actions = views::request::RequestActions::default();
        let selected = self.state.selected_request().map(|r| r.id.clone());
        let settings = self.state.ui;
        let (mut sidebar_ratio, mut request_ratio) =
            (settings.sidebar_ratio, settings.request_ratio);
        let runner_open = self.runner_open;
        let mut close_runner = false;
        let Self {
            state,
            sidebar,
            response,
            runner,
            ..
        } = self;
        // The panes run one after the other but their closures are built together.
        let shared = std::cell::RefCell::new(&mut *state);

        SplitPane::horizontal("nexus-sidebar", &mut sidebar_ratio)
            .default_ratio(UiSettings::DEFAULT_SIDEBAR)
            .min_sizes(MIN_SIDEBAR, MIN_EDITOR)
            .label("Resize sidebar")
            .show(
                &mut shell,
                |ui| {
                    island(ui, |ui| {
                        views::sidebar::show(
                            ui,
                            &mut shared.borrow_mut(),
                            sidebar,
                            &mut side_actions,
                        )
                    })
                },
                |ui| {
                    if runner_open {
                        island(ui, |ui| {
                            views::runner::show(
                                ui,
                                &mut shared.borrow_mut(),
                                runner,
                                &mut close_runner,
                            )
                        });
                        return;
                    }
                    SplitPane::horizontal("nexus-editor", &mut request_ratio)
                        .default_ratio(UiSettings::DEFAULT_REQUEST)
                        .min_size(MIN_PANE)
                        .label("Resize request and response")
                        .show(
                            ui,
                            |ui| {
                                island(ui, |ui| match &selected {
                                    Some(id) => views::request::show(
                                        ui,
                                        &mut shared.borrow_mut(),
                                        id,
                                        &mut req_actions,
                                    ),
                                    None => welcome(ui),
                                })
                            },
                            |ui| {
                                island(ui, |ui| {
                                    if let Some(id) = &selected {
                                        views::response::show(
                                            ui,
                                            &mut shared.borrow_mut(),
                                            id,
                                            response,
                                        );
                                    }
                                })
                            },
                        );
                },
            );
        if close_runner {
            self.runner_open = false;
        }
        let layout = UiSettings {
            sidebar_ratio,
            request_ratio,
        };
        if layout != settings {
            self.state.ui = layout;
            self.layout_dirty = Some(std::time::Instant::now());
        }
        // Save once the divider is let go, not on every frame of a drag.
        if self.layout_dirty.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.layout_dirty = None;
            self.state.save_ui();
        }
        toast(ui.ctx(), &mut self.state);
        if let Some(scope) = side_actions.run.take() {
            self.open_runner(scope);
        }

        if side_actions.open_import {
            self.overlays.open(Overlay::Import);
        }
        if side_actions.open_environments {
            self.overlays.open(Overlay::Environments);
        }
        if side_actions.export {
            self.overlays.open(Overlay::Export);
        }
        if req_actions.send {
            self.send_selected();
        }
        if let Some(id) = self.state.selected.clone() {
            if req_actions.connect {
                self.state.connect_stream(&id);
            }
            if req_actions.disconnect {
                self.state.disconnect_stream(&id);
            }
            if req_actions.send_message {
                self.state.send_stream_message(&id);
            }
        }
        if req_actions.cancel {
            self.state.cancel();
        }
        if req_actions.generate_code {
            self.overlays.open(Overlay::Code);
        }
        if req_actions.load_grpc_schema
            && let Some(id) = self.state.selected.clone()
        {
            self.state.load_grpc_schema(&id);
        }
        if req_actions.fetch_token {
            self.start_token_fetch();
        }
        views::overlays::show(ui, &mut self.state, &mut self.overlays);
    }
}

/// An island filling its pane.
fn island(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    // A pane can be empty (or even negative) in egui's first, sizing frame.
    let size = ui.max_rect().size().max(egui::Vec2::ZERO);
    Surface::Island
        .frame(ui.ctx())
        .inner_margin(0)
        .show(ui, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            // Views assume a usable pane: skip egui's zero-sized sizing frames.
            if size.x >= MIN_CONTENT {
                add(ui);
            }
        });
}

fn welcome(ui: &mut Ui) {
    EmptyState::new("Select or create a request")
        .icon(Glyph::Send)
        .hint("Create a request to start testing your API, or jump to one.")
        .show(ui);
    ui.add_space(8.0);
    for (shortcut, what) in [("Ctrl+N", "new request"), ("Ctrl+P", "go to request")] {
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.horizontal(|ui| {
                // Center the key caps and their caption as one row.
                ui.add_space((ui.available_width() - 150.0).max(0.0) / 2.0);
                ui.add(Kbd::parse(shortcut));
                ui.label(what);
            });
        });
    }
}

fn toast(ctx: &egui::Context, state: &mut NexusState) {
    const LIFETIME: f32 = 4.0;
    let Some((message, at)) = &state.notice else {
        return;
    };
    let age = at.elapsed().as_secs_f32();
    if age > LIFETIME {
        state.notice = None;
        return;
    }
    let severity = if message.contains("failed")
        || message.starts_with("Cannot")
        || message.starts_with("Could not")
    {
        Severity::Error
    } else {
        Severity::Info
    };
    let out = Toast::new(severity, message.as_str()).show_corner(ctx, "nexus-notice", 0);
    if out.dismissed {
        state.notice = None;
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f32(LIFETIME - age));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2, vec2};
    use nexus_http_core::model::{Body, Method, Request, Workspace};

    fn render(app: &mut NexusApp, dark: bool) {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 860.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.ui(ui));
            });
        }
    }

    #[test]
    fn renders_every_tab_and_overlay_headlessly() {
        let mut ws = Workspace::new("Demo");
        ws.items.push(Item::Request(
            Request::new("Create", Method::Post, "{{ base }}/items").with_body(Body::Json {
                text: "{\"a\": 1}".into(),
            }),
        ));
        let mut grpc = Request::new("Say", Method::Post, "grpc://localhost:50051");
        grpc.protocol = nexus_http_core::model::Protocol::Grpc;
        grpc.grpc.method = "pkg.Echo/Say".into();
        let grpc_id = grpc.id.clone();
        ws.items.push(Item::Request(grpc));
        let mut app = NexusApp::with_state(NexusState::in_memory(ws));
        app.state.grpc_schemas.entry(grpc_id.clone()).or_default();
        for selected in [None, Some(grpc_id)] {
            if let Some(id) = selected {
                app.state.selected = Some(id);
            }
            render(&mut app, true);
        }
        for tab in state::RequestTab::ALL {
            app.state.request_tab = tab;
            render(&mut app, true);
        }
        for tab in state::ResponseTab::ALL {
            app.state.response_tab = tab;
            render(&mut app, false);
        }
        for overlay in [
            Overlay::Environments,
            Overlay::Switcher,
            Overlay::Import,
            Overlay::Export,
            Overlay::Code,
        ] {
            app.open_overlay(overlay);
            render(&mut app, true);
        }
    }
}
