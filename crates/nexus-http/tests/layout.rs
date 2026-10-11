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
    let mut harness = testing::harness_state_sized(Theme::dark(), size, app, |ui, app| app.ui(ui));
    harness.run();
    harness
}

/// Tab stops the audit reports because the kit's collections hold the focus as a whole
/// (every row is "unreachable", and Tab does not continue past the sidebar to the
/// dividers after it): archipel-dev/kit#21.
fn known_kit_gap(f: &testing::Finding) -> bool {
    f.problem == Problem::Unreachable
        && (f.element.starts_with("Button") || f.element.contains("Resize right tool window"))
}

#[test]
fn app_screen_passes_the_audit_at_1280x800() {
    let mut harness = harness(app(), egui::vec2(1280.0, 800.0));
    let found: Vec<_> = testing::audit(&mut harness)
        .findings
        .iter()
        .filter(|f| !known_kit_gap(f))
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
    use archipel_kit::tokens::size;
    let mut app = app();
    // Ask for far more than fits: the layout clamps, the stored share is untouched.
    app.state.ui = UiSettings {
        sidebar_ratio: 0.9,
        request_ratio: 0.05,
    };
    let harness = harness(app, egui::vec2(1024.0, 700.0));
    let left = harness.get_by_label("Resize left tool window").rect();
    let right = harness.get_by_label("Resize right tool window").rect();
    // The request keeps the kit's minimum center width between the two tool windows.
    assert!(
        right.center().x - left.center().x >= size::WORKBENCH_CENTER_MIN,
        "{left:?} {right:?}"
    );
    assert!(left.center().x >= size::SPLIT_MIN_PANE, "{left:?}");
    assert!(
        1024.0 - right.center().x >= size::SPLIT_MIN_PANE,
        "{right:?}"
    );
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

#[test]
fn the_response_tabs_in_the_tool_window_header_switch_the_pane() {
    use nexus_http::state::{Outcome, ResponseTab};
    use nexus_http_core::response::{Response, Timings};
    let mut app = app();
    let id = app.state.selected.clone().expect("a selected request");
    let response = Response {
        request_id: None,
        method: "GET".into(),
        url: "https://api.test/users".into(),
        status: 200,
        reason: "OK".into(),
        http_version: "HTTP/1.1".into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: b"{}".to_vec(),
        truncated: false,
        timings: Timings::default(),
        timeline: Vec::new(),
        redirects: 0,
        started_at: 0,
    };
    app.state
        .outcomes
        .insert(id, Outcome::Response(Box::new(response), Vec::new()));
    let mut harness = harness(app, egui::vec2(1600.0, 800.0));
    harness.get_by_label("Headers (1)").click();
    harness.run();
    assert_eq!(harness.state().state.response_tab, ResponseTab::Headers);
}
