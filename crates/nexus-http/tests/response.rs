//! Behavior, conformance and audit tests for the response pane.

use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::{Button, Dropdown, IconButton, TextField, Tooltip};
use archipel_kit::testing::{self, NodeT, Queryable, Role};
use archipel_kit::{Glyph, Theme, egui};
use nexus_http::state::{NexusState, Outcome, ResponseTab};
use nexus_http::views::response::{self, ResponseUi};
use nexus_http_core::assertions::AssertionResult;
use nexus_http_core::model::{Id, Method, Request, Workspace};
use nexus_http_core::response::{Response, TimelineEntry, TimelineKind, Timings};

fn response(status: u16, body: &str, started_at: u64) -> Response {
    Response {
        request_id: None,
        method: "GET".into(),
        url: "http://localhost/items".into(),
        status,
        reason: "OK".into(),
        http_version: "HTTP/1.1".into(),
        headers: vec![
            ("content-type".into(), "application/json".into()),
            ("x-request-id".into(), "abc-123".into()),
            ("set-cookie".into(), "session=1; Path=/".into()),
        ],
        body: body.as_bytes().to_vec(),
        truncated: false,
        timings: Timings {
            prepare_ms: 1.0,
            wait_ms: 10.0,
            download_ms: 2.0,
            total_ms: 13.0,
        },
        timeline: vec![
            TimelineEntry::new(TimelineKind::Info, "Connected"),
            TimelineEntry::new(TimelineKind::RequestHeader, "GET /items HTTP/1.1"),
            TimelineEntry::new(TimelineKind::ResponseHeader, "HTTP/1.1 200 OK"),
        ],
        redirects: 0,
        started_at,
    }
}

const BODY: &str = r#"{"items":[{"name":"alpha"},{"name":"beta"}],"count":2}"#;

struct Fixture {
    state: NexusState,
    ui: ResponseUi,
    id: Id,
}

fn fixture() -> Fixture {
    let mut workspace = Workspace::new("Test");
    let request = Request::new("Items", Method::Get, "http://localhost/items");
    let id = request.id.clone();
    workspace
        .items
        .push(nexus_http_core::model::Item::Request(request));
    let mut state = NexusState::in_memory(workspace);
    let checks = vec![
        AssertionResult {
            description: "status equals 200".into(),
            passed: true,
            actual: "200".into(),
        },
        AssertionResult {
            description: "count equals 3".into(),
            passed: false,
            actual: "2".into(),
        },
    ];
    state.outcomes.insert(
        id.clone(),
        Outcome::Response(Box::new(response(200, BODY, 0)), checks),
    );
    Fixture {
        state,
        ui: ResponseUi::default(),
        id,
    }
}

/// Whether any text on screen contains `needle` (body text is drawn as text runs).
fn shows_text(h: &testing::Harness<'_, Fixture>, needle: &str) -> bool {
    h.get_all_by(|_| true).any(|n| {
        let d = n.accesskit_node();
        d.value().is_some_and(|v| v.contains(needle))
    })
}

fn harness(fixture: Fixture) -> testing::Harness<'static, Fixture> {
    let mut harness = testing::harness_state(Theme::dark(), fixture, |ui, f| {
        response::show(ui, &mut f.state, &f.id, &mut f.ui);
    });
    harness.set_size(egui::vec2(900.0, 600.0));
    harness.run_ok();
    harness
}

#[test]
fn audit_every_tab() {
    for tab in ResponseTab::ALL {
        let mut f = fixture();
        f.state.response_tab = tab;
        let mut h = harness(f);
        let audit = testing::audit(&mut h);
        assert!(audit.is_clean(), "{tab:?}: {:?}", audit.findings);
    }
}

#[test]
fn audit_error_and_idle_states() {
    let mut f = fixture();
    f.state.outcomes.insert(
        f.id.clone(),
        Outcome::Error("could not connect to localhost".into()),
    );
    let mut h = harness(f);
    h.get_by_label_contains("could not be completed");
    testing::audit(&mut h).assert_clean();

    let mut f = fixture();
    f.state.outcomes.clear();
    let mut h = harness(f);
    h.get_by_label("No response yet");
    testing::audit(&mut h).assert_clean();
}

#[test]
fn status_time_and_size_are_tags() {
    let h = harness(fixture());
    h.get_by_label("200 OK");
    h.get_by_label("13 ms");
}

#[test]
fn jsonpath_filter_narrows_the_preview() {
    let mut h = harness(fixture());
    assert!(shows_text(&h, "alpha"));
    let input = h.get_by_label("JSONPath filter");
    input.focus();
    input.type_text("$.count");
    h.run_ok();
    assert_eq!(h.state().ui.filter, "$.count");
    assert!(!shows_text(&h, "alpha"));
}

#[test]
fn invalid_filter_shows_a_banner_not_a_panic() {
    let mut h = harness(fixture());
    let input = h.get_by_label("JSONPath filter");
    input.focus();
    input.type_text("$[");
    h.run_ok();
    testing::audit(&mut h).assert_clean();
}

#[test]
fn switching_tabs_shows_headers() {
    let mut h = harness(fixture());
    h.get_by_label("Headers (3)").click();
    h.run_ok();
    assert_eq!(h.state().state.response_tab, ResponseTab::Headers);
    h.get_by_role_and_label(Role::Table, "Response headers");
    h.get_by_label_contains("x-request-id");
}

#[test]
fn tests_tab_lists_assertions() {
    let mut h = harness(fixture());
    h.get_by_label("Tests (1/2)").click();
    h.run_ok();
    h.get_by_label_contains("status equals 200");
    h.get_by_label_contains("count equals 3");
}

#[test]
fn timeline_tab_lists_the_exchange() {
    let mut h = harness(fixture());
    h.get_by_label("Timeline").click();
    h.run_ok();
    h.get_by_label_contains("GET /items HTTP/1.1");
}

#[test]
fn history_dropdown_switches_to_an_older_response() {
    let mut f = fixture();
    let id = f.id.clone();
    // Newest first when read back: push the older one first.
    f.state.history.push(id.clone(), response(500, "{}", 0));
    f.state.history.push(id, response(200, BODY, 1));
    f.state.outcomes.clear();
    let mut h = harness(f);
    h.set_size(egui::vec2(900.0, 700.0));
    h.run_ok();
    h.get_by_role(Role::ComboBox).click();
    h.run_ok();
    h.get_all_by_label_contains("500").next().unwrap().click();
    h.run_ok();
    assert_eq!(h.state().ui.history_index, Some(1));
}

#[test]
fn copy_button_has_a_name() {
    let h = harness(fixture());
    h.get_by_label("Copy body");
    h.get_by_label("Save body to file");
}

// -- conformance: the interactive elements this view adds, built the way the view builds them

#[test]
fn copy_and_save_buttons_conform() {
    testing::conformance("Copy body", |ui, enabled| {
        ui.add(
            IconButton::new(Glyph::Copy, "Copy body")
                .enabled(enabled)
                .with(Tooltip::new("Copy body")),
        )
    })
    .run();
    testing::conformance("Save body to file", |ui, enabled| {
        ui.add(
            IconButton::new(Glyph::Save, "Save body to file")
                .enabled(enabled)
                .with(Tooltip::new("Save body to file")),
        )
    })
    .run();
}

#[test]
fn filter_field_conforms() {
    let mut text = String::new();
    testing::conformance("JSONPath filter", move |ui, enabled| {
        ui.add(
            TextField::search(&mut text)
                .label("JSONPath filter")
                .monospace(true)
                .enabled(enabled),
        )
    })
    .no_click()
    .keys(&[])
    .run();
}

#[test]
fn clear_buttons_conform() {
    testing::conformance("Clear all", |ui, enabled| {
        ui.add(Button::ghost("Clear all").small(true).enabled(enabled))
    })
    .run();
    testing::conformance("Clear", |ui, enabled| {
        ui.add(Button::secondary("Clear").enabled(enabled))
    })
    .run();
}

#[test]
fn history_dropdown_conforms() {
    let mut value = "200 OK · 13 ms".to_owned();
    testing::conformance("Response history", move |ui, enabled| {
        ui.add(
            Dropdown::new(
                "response-history",
                &mut value,
                ["200 OK · 13 ms".to_owned(), "500 Error · 4 ms".to_owned()],
            )
            .label("Response history")
            .enabled(enabled),
        )
    })
    .run();
}
