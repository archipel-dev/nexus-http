//! The standalone window's main toolbar, like Android Studio's: conformance of its widgets,
//! the audit and snapshots of the whole window, and what its menus do.
//!
//! The snapshots show the toolbar as Linux and Windows draw it. On macOS the same row also
//! holds the native traffic lights (`WindowChrome::Integrated`); those belong to the OS and
//! can't be rendered here.

use archipel_kit::components::MenuItem;
use archipel_kit::testing::{self, Harness, NodeT, Problem, Queryable, Role, SnapshotOptions};
use archipel_kit::{Theme, egui};
use nexus_http::NexusApp;
use nexus_http::state::NexusState;
use nexus_http::views::toolbar;
use nexus_http_core::model::{Environment, Workspace};

fn app() -> NexusApp {
    let mut workspace = Workspace::new("Acme Store API");
    let local = Environment::new("Local");
    workspace.active_environment = Some(local.id.clone());
    workspace.environments = vec![local, Environment::new("Staging")];
    let mut state = NexusState::in_memory(workspace);
    let users = state.new_request(None);
    state.rename(&users, "List users");
    state.request_mut(&users).unwrap().url = "https://api.test/users".into();
    state.selected = Some(users);
    NexusApp::with_state(state)
}

/// The whole standalone window (`NexusApp::window`) at `size`.
fn window(app: NexusApp, theme: Theme, size: egui::Vec2, scale: f32) -> Harness<'static, NexusApp> {
    let mut theme = Some(theme);
    let mut harness = testing::builder()
        .with_size(size)
        .with_pixels_per_point(scale)
        .build_state(
            move |ctx, app: &mut NexusApp| {
                // Fonts installed during a frame take effect on the next one.
                if let Some(theme) = theme.take() {
                    archipel_kit::install(ctx, theme);
                    ctx.request_repaint();
                    return;
                }
                app.window(ctx);
            },
            app,
        );
    harness.run_ok();
    harness
}

fn screen(app: NexusApp) -> Harness<'static, NexusApp> {
    window(app, Theme::dark(), egui::vec2(1280.0, 800.0), 1.0)
}

/// The kit's known gap (archipel-dev/kit#21): rows of its collections hold the focus as a
/// whole, so Tab doesn't stop on each one or continue to the dividers after them.
fn known_kit_gap(f: &testing::Finding) -> bool {
    f.problem == Problem::Unreachable
        && (f.element.starts_with("Button") || f.element.contains("Resize right tool window"))
}

#[test]
fn toolbar_widgets_conform() {
    testing::conformance("Acme Store API", |ui, enabled| {
        toolbar::workspace_button("Acme Store API", false)
            .enabled(enabled)
            .show(ui)
    })
    .run();
    testing::conformance("Staging", |ui, enabled| {
        toolbar::environment_button("Staging", false)
            .enabled(enabled)
            .show(ui)
    })
    .run();
    testing::conformance("Go to request", |ui, enabled| {
        ui.add(toolbar::go_to_request_button().enabled(enabled))
    })
    .run();
    testing::conformance("Run collection", |ui, enabled| {
        ui.add(toolbar::run_collection_button().enabled(enabled))
    })
    .run();
    // The menus' rows.
    for label in [
        "Export workspace…",
        "Run collection…",
        "Manage environments…",
    ] {
        testing::conformance(label, move |ui, enabled| {
            ui.add(MenuItem::new(label).enabled(enabled))
        })
        .run();
    }
    testing::conformance("Staging", |ui, enabled| {
        ui.add(MenuItem::new("Staging").checked(false).enabled(enabled))
    })
    .run();
}

#[test]
fn the_window_with_its_toolbar_passes_the_audit_in_both_themes() {
    for theme in [Theme::dark(), Theme::light()] {
        let mut harness = window(app(), theme, egui::vec2(1280.0, 800.0), 1.0);
        assert!(
            harness
                .query_by_role_and_label(Role::ComboBox, "Acme Store API")
                .is_some(),
            "the workspace widget is in the toolbar"
        );
        let found: Vec<_> = testing::audit(&mut harness)
            .findings
            .iter()
            .filter(|f| !known_kit_gap(f))
            .map(|f| format!("{:?}: {} {}", f.problem, f.element, f.detail))
            .collect();
        assert!(found.is_empty(), "{found:#?}");
    }
}

/// Keyboard only: Tab reaches the workspace widget, Enter opens its menu, Escape closes it.
#[test]
fn the_workspace_menu_opens_from_the_keyboard() {
    let mut harness = screen(app());
    let focused = |h: &Harness<'_, NexusApp>| {
        h.get_by_role_and_label(Role::ComboBox, "Acme Store API")
            .accesskit_node()
            .is_focused()
    };
    for _ in 0..60 {
        if focused(&harness) {
            break;
        }
        harness.key_press(egui::Key::Tab);
        harness.run_ok();
    }
    assert!(focused(&harness), "Tab reaches the workspace widget");
    harness.key_press(egui::Key::Enter);
    harness.run_ok();
    assert!(harness.query_by_label("Export workspace…").is_some());
    harness.key_press(egui::Key::Escape);
    harness.run_ok();
    assert!(harness.query_by_label("Export workspace…").is_none());
}

#[test]
fn window_snapshots() {
    let mut results = testing::SnapshotResults::new();
    for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let mut harness = window(app(), theme, egui::vec2(1100.0, 560.0), 2.0);
        results.add(harness.try_snapshot_options(
            format!("window_toolbar.{name}"),
            &SnapshotOptions::new().output_path("tests/snapshots"),
        ));
        // A real click on the workspace widget. AccessKit bounds are physical pixels: at 2×
        // the node's center is halved to reach the widget in points.
        let center = harness
            .get_by_role_and_label(Role::ComboBox, "Acme Store API")
            .rect()
            .center();
        let at = egui::pos2(center.x / 2.0, center.y / 2.0);
        harness.hover_at(at);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
        harness.run_ok();
        assert!(harness.query_by_label("Export workspace…").is_some());
        results.add(harness.try_snapshot_options(
            format!("window_workspace_menu.{name}"),
            &SnapshotOptions::new().output_path("tests/snapshots"),
        ));
    }
    results.unwrap();
}

#[test]
fn the_workspace_menu_runs_the_workspace_actions() {
    let mut harness = screen(app());
    assert!(harness.query_by_label("Export workspace…").is_none());
    harness
        .get_by_role_and_label(Role::ComboBox, "Acme Store API")
        .click();
    harness.run_ok();
    for item in ["Import…", "Run collection…", "Manage environments…"] {
        assert!(harness.query_by_label(item).is_some(), "{item}");
    }
    harness.get_by_label("Export workspace…").click();
    harness.run_ok();
    assert!(
        harness.query_by_label("Export Workspace").is_some(),
        "the export dialog opened"
    );
    assert!(
        harness.query_by_label("Run collection…").is_none(),
        "picking an item closes the menu"
    );

    // Escape closes the menu without doing anything.
    let mut harness = screen(app());
    harness
        .get_by_role_and_label(Role::ComboBox, "Acme Store API")
        .click();
    harness.run_ok();
    harness.key_press(egui::Key::Escape);
    harness.run_ok();
    assert!(harness.query_by_label("Export workspace…").is_none());
    assert!(!harness.state().runner_open);

    let mut harness = screen(app());
    harness
        .get_by_role_and_label(Role::ComboBox, "Acme Store API")
        .click();
    harness.run_ok();
    harness.get_by_label("Run collection…").click();
    harness.run_ok();
    assert!(harness.state().runner_open, "the collection runner opened");
}

#[test]
fn the_environment_widget_switches_the_active_environment() {
    let mut harness = screen(app());
    // The window shows the environment in the toolbar, not in the collections tool window.
    assert!(harness.query_by_label("Environment").is_none());
    harness
        .get_by_role_and_label(Role::ComboBox, "Local")
        .click();
    harness.run_ok();
    harness.get_by_label("Staging").click();
    harness.run_ok();
    let state = &harness.state().state;
    assert_eq!(
        state
            .workspace
            .active_environment()
            .map(|e| e.name.as_str()),
        Some("Staging")
    );
    assert!(
        harness
            .query_by_role_and_label(Role::ComboBox, "Staging")
            .is_some()
    );
    assert!(
        harness
            .query_by_role_and_label(Role::ComboBox, "Local")
            .is_none()
    );

    // And back to the base environment.
    harness
        .get_by_role_and_label(Role::ComboBox, "Staging")
        .click();
    harness.run_ok();
    harness.get_by_label(toolbar::BASE_ENVIRONMENT).click();
    harness.run_ok();
    assert!(harness.state().state.workspace.active_environment.is_none());
}

#[test]
fn the_search_button_opens_go_to_request() {
    let mut harness = screen(app());
    let before = harness.query_all_by_label("Go to request").count();
    harness
        .get_by_role_and_label(Role::Button, "Go to request")
        .click();
    harness.run_ok();
    assert!(
        harness.query_all_by_label("Go to request").count() > before,
        "the Go to request dialog opened"
    );
}

#[test]
fn an_embedded_client_has_no_toolbar_and_keeps_its_environment_picker() {
    let mut harness = testing::harness_state_sized(
        Theme::dark(),
        egui::vec2(1280.0, 800.0),
        app(),
        |ui, app| app.ui(ui),
    );
    harness.run_ok();
    assert!(
        harness
            .query_by_role_and_label(Role::ComboBox, "Acme Store API")
            .is_none()
    );
    assert!(harness.query_by_label("Environment").is_some());
}
