//! The app shell on archipel-kit: audit, split behaviour and the toast.

use archipel_kit::Theme;
use archipel_kit::egui;
use archipel_kit::testing::{self, Harness, Problem, Queryable, Role};

use nexus_http::NexusApp;
use nexus_http::state::{NexusState, UiSettings};
use nexus_http_core::model::Workspace;

fn app() -> NexusApp {
    let mut state = NexusState::in_memory(Workspace::new("Acme"));
    let users = state.new_request(None);
    state.rename(&users, "List users");
    state.request_mut(&users).unwrap().url = "https://api.test/users".into();
    let invoice = state.new_request(None);
    state.rename(&invoice, "Get invoice");
    state.request_mut(&invoice).unwrap().url = "https://api.test/invoices/1".into();
    state.selected = Some(users);
    NexusApp::with_state(state)
}

fn harness(app: NexusApp, size: egui::Vec2) -> Harness<'static, NexusApp> {
    let mut harness = testing::harness_state(Theme::dark(), app, |ui, app| app.ui(ui));
    harness.set_size(size);
    harness.run();
    harness
}

/// Tab stops the audit reports because the kit's collections hold the focus as a whole
/// (every row is "unreachable", and Tab does not continue past the sidebar to the
/// dividers after it): archipel-dev/kit#21.
fn known_kit_gap(f: &testing::Finding) -> bool {
    f.problem == Problem::Unreachable
        && (f.element.starts_with("Button") || f.element.contains("Resize request and response"))
}

#[test]
fn app_screen_passes_the_audit_at_1280x800() {
    let mut harness = harness(app(), egui::vec2(1280.0, 800.0));
    let found: Vec<_> = testing::audit(&mut harness)
        .findings
        .iter()
        .filter(|f| {
            // The legacy request and response panes (widgets.rs, theme.rs) are ported in
            // archipel-dev/nexus-http#4.
            let legacy = f.element.contains("TextInput")
                || f.element.contains("Unknown")
                || f.element.starts_with("Label")
                || f.detail.contains("Unknown");
            !(known_kit_gap(f) || legacy)
        })
        .map(|f| format!("{:?}: {} {}", f.problem, f.element, f.detail))
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn welcome_screen_passes_the_audit() {
    let mut app = app();
    app.state.selected = None;
    let mut harness = harness(app, egui::vec2(1280.0, 800.0));
    let found: Vec<_> = testing::audit(&mut harness)
        .findings
        .iter()
        .filter(|f| !known_kit_gap(f))
        .map(|f| format!("{:?}: {} {}", f.problem, f.element, f.detail))
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn split_ratios_clamp_to_the_min_sizes_on_a_narrow_window() {
    let mut app = app();
    // Ask for far more than fits: the layout clamps, the stored share is untouched.
    app.state.ui = UiSettings {
        sidebar_ratio: 0.9,
        request_ratio: 0.05,
    };
    let harness = harness(app, egui::vec2(1024.0, 700.0));
    let sidebar = harness.get_by_label("Resize sidebar").rect();
    let editor = harness.get_by_label("Resize request and response").rect();
    // 1024 wide: the sidebar keeps at least 200, the editor at least 560 (2 x 280).
    assert!(sidebar.center().x <= 1024.0 - 560.0 + 4.0, "{sidebar:?}");
    assert!(sidebar.center().x >= 200.0, "{sidebar:?}");
    assert!(
        editor.center().x - sidebar.center().x >= 280.0,
        "{editor:?} {sidebar:?}"
    );
    assert!(1024.0 - editor.center().x >= 280.0, "{editor:?}");
}

#[test]
fn a_finished_send_shows_a_toast() {
    let mut app = app();
    app.state.notify("200 OK");
    let mut harness = harness(app, egui::vec2(1280.0, 800.0));
    assert!(
        harness
            .query_by_role_and_label(Role::Label, "200 OK")
            .is_some()
    );
    harness.get_by_label("Dismiss").click();
    harness.run();
    assert!(harness.state().state.notice.is_none());
}

#[test]
fn the_welcome_state_shows_its_shortcuts() {
    let mut app = app();
    app.state.selected = None;
    let harness = harness(app, egui::vec2(1280.0, 800.0));
    assert!(
        harness
            .query_by_role_and_label(Role::Label, "Select or create a request")
            .is_some()
    );
}

#[test]
fn layout_persists_next_to_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = NexusState::open(dir.path());
    state.ui.sidebar_ratio = 0.3;
    state.save_ui();
    assert_eq!(NexusState::open(dir.path()).ui.sidebar_ratio, 0.3);
    // A file with missing fields falls back to the defaults.
    std::fs::write(dir.path().join(".nexus/ui.json"), "{}").unwrap();
    assert_eq!(NexusState::open(dir.path()).ui, UiSettings::default());
}
