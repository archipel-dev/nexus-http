//! Behavior, conformance and audit tests for the collection runner.

use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::{Button, IconButton, Tooltip};
use archipel_kit::testing::{self, Queryable, Role};
use archipel_kit::{Glyph, Theme, egui};
use nexus_http::state::NexusState;
use nexus_http::views::runner::{self, RunnerUi};
use nexus_http_core::assertions::{Assertion, Operator, Source};
use nexus_http_core::model::{Item, Method, Request, Workspace};
use std::time::Duration;

struct Screen {
    state: NexusState,
    runner: RunnerUi,
    closed: bool,
}

/// A local server: `/ok*` answers 200, anything else 500.
fn server() -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let code = if request.url().starts_with("/ok") {
                200
            } else {
                500
            };
            let _ = request.respond(tiny_http::Response::from_string("{}").with_status_code(code));
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn screen(base: &str) -> Screen {
    let mut workspace = Workspace::new("Test");
    for (name, path) in [("First", "/ok1"), ("Second", "/ok2"), ("Broken", "/broken")] {
        let mut request = Request::new(name, Method::Get, format!("{base}{path}"));
        request
            .assertions
            .push(Assertion::new(Source::Status, Operator::Equals, "200"));
        workspace.items.push(Item::Request(request));
    }
    Screen {
        state: NexusState::in_memory(workspace),
        runner: RunnerUi::default(),
        closed: false,
    }
}

fn harness(screen: Screen) -> testing::Harness<'static, Screen> {
    let mut harness = testing::harness_state(Theme::dark(), screen, |ui, s| {
        let mut close = false;
        runner::show(ui, &mut s.state, &mut s.runner, &mut close);
        s.closed |= close;
    });
    harness.set_size(egui::vec2(1000.0, 700.0));
    harness.run_ok();
    harness
}

fn run_to_completion(h: &mut testing::Harness<'static, Screen>) {
    h.get_by_label("Run 3 requests").click();
    for _ in 0..400 {
        h.run_ok();
        if h.query_by_label_contains("passed").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the run did not finish");
}

#[test]
fn audit_planned_screen() {
    let mut h = harness(screen("http://127.0.0.1:9"));
    h.get_by_role_and_label(Role::Table, "Requests to run");
    testing::audit(&mut h).assert_clean();
}

#[test]
fn run_reports_pass_and_fail_counts() {
    let base = server();
    let mut h = harness(screen(&base));
    run_to_completion(&mut h);
    h.get_by_label("2 passed");
    h.get_by_label("1 failed");
    h.get_by_label("2/3 assertions");
    h.get_by_role_and_label(Role::Table, "Run results");
    h.get_by_label_contains("Pass, GET, First");
    h.get_by_label_contains("Fail, GET, Broken");
    // The first failure is selected, so its assertion details are shown.
    h.get_by_label_contains("Broken · details");
}

#[test]
fn audit_results_screen() {
    let base = server();
    let mut h = harness(screen(&base));
    run_to_completion(&mut h);
    testing::audit(&mut h).assert_clean();
}

#[test]
fn close_button_reports_close() {
    let mut h = harness(screen("http://127.0.0.1:9"));
    h.get_by_label("Back to requests").click();
    h.run_ok();
    assert!(h.state().closed);
}

#[test]
fn empty_scope_disables_run() {
    let mut s = screen("http://127.0.0.1:9");
    s.state.workspace.items.clear();
    let h = harness(s);
    assert!(testing::NodeExt::is_disabled(
        &h.get_by_label_contains("Run 0 request")
    ));
    h.get_by_label("Nothing to run");
}

#[test]
fn runner_buttons_conform() {
    testing::conformance("Run 3 requests", |ui, enabled| {
        ui.add(
            Button::primary("Run 3 requests")
                .icon(Glyph::Run)
                .enabled(enabled),
        )
    })
    .run();
    testing::conformance("Stop", |ui, enabled| {
        ui.add(Button::secondary("Stop").icon(Glyph::Stop).enabled(enabled))
    })
    .run();
    testing::conformance("Back to requests", |ui, enabled| {
        ui.add(
            IconButton::new(Glyph::Close, "Back to requests")
                .enabled(enabled)
                .with(Tooltip::new("Back to requests").shortcut("Esc")),
        )
    })
    .run();
}

#[test]
fn streaming_requests_show_as_streamed_not_failed() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let _ = std::io::Read::read(&mut s, &mut [0u8; 2048]);
        let _ = s.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: one\n\ndata: two\n\n",
        );
        let _ = s.flush();
        std::thread::sleep(Duration::from_secs(10));
    });
    let mut request = Request::new("Ticker", Method::Get, format!("http://127.0.0.1:{port}/e"));
    request.protocol = nexus_http_core::model::Protocol::Sse;
    request
        .assertions
        .push(Assertion::new(Source::Body, Operator::Contains, "one"));
    let mut workspace = Workspace::new("Test");
    workspace.items.push(Item::Request(request));
    let mut h = harness(Screen {
        state: NexusState::in_memory(workspace),
        runner: RunnerUi::default(),
        closed: false,
    });
    h.get_by_label("Run 1 request").click();
    for _ in 0..400 {
        h.run_ok();
        if h.query_by_label_contains("passed").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    h.get_by_label("1 passed");
    h.get_by_label("1 streamed");
    assert!(h.query_by_label("1 failed").is_none());
    h.get_by_label_contains("Streamed, SSE, Ticker");
    testing::audit(&mut h).assert_clean();
}
