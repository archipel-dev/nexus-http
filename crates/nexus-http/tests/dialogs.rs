//! Dialogs on archipel-kit: audit, conformance of their controls and keyboard behaviour.

use archipel_kit::components::{Banner, Button, Checkbox, Dialog, Severity};
use archipel_kit::egui;
use archipel_kit::testing::{self, Harness, Problem, Queryable, Role};
use archipel_kit::{Glyph, Theme};
use nexus_http::state::NexusState;
use nexus_http::views::overlays::{self, Overlay, OverlayUi};
use nexus_http_core::model::Workspace;

struct Screen {
    state: NexusState,
    overlays: OverlayUi,
}

fn state() -> NexusState {
    let mut state = NexusState::in_memory(Workspace::new("Acme"));
    let users = state.new_request(None);
    state.rename(&users, "List users");
    state.request_mut(&users).unwrap().url = "https://api.test/users".into();
    let invoice = state.new_request(None);
    state.rename(&invoice, "Get invoice");
    state.request_mut(&invoice).unwrap().url = "https://api.test/invoices/1".into();
    state.selected = Some(users);
    state
}

fn open(overlay: Overlay) -> Harness<'static, Screen> {
    let mut overlays = OverlayUi::default();
    overlays.open(overlay);
    let mut harness = testing::harness_state_sized(
        Theme::dark(),
        egui::vec2(1280.0, 800.0),
        Screen {
            state: state(),
            overlays,
        },
        |ui, s| overlays::show(ui, &mut s.state, &mut s.overlays),
    );
    harness.run();
    harness
}

const ALL: [(Overlay, &str); 5] = [
    (Overlay::Switcher, "Go to request"),
    (Overlay::Environments, "Manage Environments"),
    (Overlay::Import, "Import"),
    (Overlay::Export, "Export Workspace"),
    (Overlay::Code, "Generate Code"),
];

/// The findings of the audit that are not the kit's own known gaps: rows of its
/// collections hold the focus as a whole (so Tab skips each row), and the legacy code
/// widgets (`TODO(kit CodeEditor)`) are not named.
fn problems(harness: &mut Harness<'_, Screen>) -> Vec<String> {
    testing::audit(harness)
        .findings
        .iter()
        .filter(|f| {
            let row = f.problem == Problem::Unreachable && f.element.starts_with("Button");
            let legacy_editor = f.element.contains("TextInput")
                || f.element.contains("Unknown")
                || f.element.starts_with("Label");
            let legacy_overlap = f.problem == Problem::Overlap && f.detail.contains("Unknown");
            !(row || legacy_editor && f.problem == Problem::MissingName || legacy_overlap)
        })
        .map(|f| format!("{:?}: {} {}", f.problem, f.element, f.detail))
        .collect()
}

#[test]
fn every_dialog_passes_the_audit() {
    for (overlay, title) in ALL {
        let mut harness = open(overlay);
        assert!(
            harness
                .query_by_role_and_label(Role::Label, title)
                .is_some(),
            "{title} is shown"
        );
        let found = problems(&mut harness);
        assert!(found.is_empty(), "{title}: {found:#?}");
    }
}

#[test]
fn escape_closes_every_dialog() {
    for (overlay, title) in ALL {
        let mut harness = open(overlay);
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(
            harness.state().overlays.open == Overlay::None,
            "{title} closes on Escape"
        );
        assert!(
            harness
                .query_by_role_and_label(Role::Label, title)
                .is_none()
        );
    }
}

#[test]
fn dialog_buttons_conform() {
    for label in ["Open", "Import", "Export", "Copy"] {
        testing::conformance(label, move |ui, enabled| {
            ui.add(Button::primary(label).enabled(enabled))
        })
        .run();
    }
    for label in ["Cancel", "Close", "Load file", "New environment"] {
        testing::conformance(label, move |ui, enabled| {
            ui.add(Button::secondary(label).enabled(enabled))
        })
        .run();
    }
    testing::conformance("Delete", |ui, enabled| {
        ui.add(Button::danger("Delete").enabled(enabled))
    })
    .run();
    let mut merge = false;
    testing::conformance("Add to the current workspace", move |ui, enabled| {
        ui.add_enabled(
            enabled,
            Checkbox::new(&mut merge, "Add to the current workspace"),
        )
    })
    .run();
}

#[test]
fn error_banner_is_audited_with_its_dialog() {
    let mut harness = testing::harness(Theme::dark(), |ui| {
        Banner::new(Severity::Error, "Invalid JSON: expected value").show(ui);
        let _ = Dialog::new("x").icon(Glyph::Warning);
    });
    testing::audit(&mut harness).assert_clean();
}

#[test]
fn enter_in_the_switcher_opens_the_request() {
    let mut harness = open(Overlay::Switcher);
    harness.state_mut().state.selected = None;
    let field = harness.get_by_role(Role::TextInput);
    field.focus();
    field.type_text("invoice");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    let s = harness.state();
    assert!(s.overlays.open == Overlay::None);
    let selected = s.state.selected.clone().expect("selection");
    assert_eq!(
        s.state.workspace.find_request(&selected).unwrap().1.name,
        "Get invoice"
    );
}

#[test]
fn arrow_down_then_enter_in_the_switcher_opens_the_second_request() {
    let mut harness = open(Overlay::Switcher);
    harness.state_mut().state.selected = None;
    harness.get_by_role(Role::TextInput).focus();
    harness.run();
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    let s = harness.state();
    let selected = s.state.selected.clone().expect("selection");
    let name = &s.state.workspace.find_request(&selected).unwrap().1.name;
    assert_eq!(name, "List users", "the cursor moved to the second match");
}

#[test]
fn import_of_a_curl_command_adds_a_request() {
    let mut harness = open(Overlay::Import);
    let before = harness.state().state.workspace.requests().len();
    let editor = harness.get_by_label("Collection to import");
    editor.focus();
    editor.type_text("curl -X POST https://api.test/orders -H 'Content-Type: application/json'");
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Import")
        .click();
    harness.run();
    let s = harness.state();
    assert_eq!(s.state.workspace.requests().len(), before + 1);
    assert!(
        s.overlays.open == Overlay::Import,
        "stays open to show the result"
    );
    assert!(
        harness
            .query_by_label_contains("Imported 1 request")
            .is_some()
    );
}

#[test]
fn closing_the_environment_editor_keeps_edits() {
    let mut harness = open(Overlay::Environments);
    harness.get_by_label("New environment").click();
    harness.run();
    assert_eq!(harness.state().state.workspace.environments.len(), 1);
    harness.get_by_label("Close").click();
    harness.run();
    assert!(harness.state().overlays.open == Overlay::None);
}

#[test]
fn export_writes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.nexus.json");
    let mut harness = open(Overlay::Export);
    let field = harness.get_by_role(Role::TextInput);
    field.focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    field.type_text(path.to_str().unwrap());
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Export")
        .click();
    harness.run();
    assert!(path.exists());
    assert!(harness.state().overlays.open == Overlay::None);
}
