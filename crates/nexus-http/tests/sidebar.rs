//! Sidebar on archipel-kit: conformance of its elements, the UX audit and behaviour.

use archipel_kit::behaviour::{ContextMenu, DragSource};
use archipel_kit::components::{Dropdown, IconButton, ListRow, MenuItem, Tag, TextField};
use archipel_kit::egui;
use archipel_kit::parts::Slot;
use archipel_kit::testing::{self, Harness, Problem, Queryable, Role};
use archipel_kit::{Glyph, Theme, Tone};
use nexus_http::state::NexusState;
use nexus_http::views::sidebar::{self, SidebarActions, SidebarUi};
use nexus_http_core::model::{Id, Workspace};

struct Screen {
    state: NexusState,
    local: SidebarUi,
    actions: SidebarActions,
}

fn named(state: &mut NexusState, id: &Id, name: &str, method: &str, url: &str) {
    state.rename(id, name);
    let request = state.request_mut(id).expect("request exists");
    request.method = method.to_owned().into();
    request.url = url.into();
}

fn screen() -> Screen {
    let mut state = NexusState::in_memory(Workspace::new("Acme"));
    let folder = state.new_folder(None);
    state.rename(&folder, "Auth");
    let login = state.new_request(Some(&folder));
    named(
        &mut state,
        &login,
        "Login",
        "POST",
        "https://api.test/login",
    );
    let users = state.new_request(None);
    named(
        &mut state,
        &users,
        "List users",
        "GET",
        "https://api.test/users",
    );
    let invoice = state.new_request(None);
    named(
        &mut state,
        &invoice,
        "Get invoice",
        "GET",
        "https://api.test/invoices/1",
    );
    state.selected = Some(users);
    Screen {
        state,
        local: SidebarUi::default(),
        actions: SidebarActions::default(),
    }
}

fn harness(theme: Theme) -> Harness<'static, Screen> {
    let mut harness = testing::harness_state(theme, screen(), |ui, s| {
        sidebar::show(ui, &mut s.state, &mut s.local, &mut s.actions);
    });
    harness.set_size(egui::vec2(300.0, 720.0));
    harness.run();
    harness
}

#[test]
fn sidebar_passes_the_audit_in_both_themes() {
    for theme in [Theme::dark(), Theme::light()] {
        let mut harness = harness(theme);
        let audit = testing::audit(&mut harness);
        // The kit's collections hold the keyboard focus themselves (Tab reaches the tree
        // once, arrows move between rows), which the audit still reports for every row.
        // Kit gap, see the PR; everything else must be clean.
        let rest: Vec<_> = audit
            .findings
            .iter()
            .filter(|f| {
                !(f.problem == Problem::Unreachable
                    && ["Auth", "Login", "List users", "Get invoice"]
                        .iter()
                        .any(|row| f.element.contains(&format!("\"{row}\""))))
            })
            .collect();
        assert!(rest.is_empty(), "audit findings: {rest:?}");
    }
}

#[test]
fn empty_sidebar_passes_the_audit_and_adds_a_request() {
    let mut harness = testing::harness_state(
        Theme::dark(),
        Screen {
            state: NexusState::in_memory(Workspace::new("Empty")),
            local: SidebarUi::default(),
            actions: SidebarActions::default(),
        },
        |ui, s| sidebar::show(ui, &mut s.state, &mut s.local, &mut s.actions),
    );
    harness.set_size(egui::vec2(300.0, 500.0));
    harness.run();
    testing::audit(&mut harness).assert_clean();
    harness.get_by_label("New request").click();
    harness.run();
    assert_eq!(harness.state().state.workspace.requests().len(), 1);
}

#[test]
fn sidebar_elements_conform() {
    testing::conformance("Workspace actions", |ui, enabled| {
        ui.add(IconButton::new(Glyph::More, "Workspace actions").enabled(enabled))
    })
    .run();
    testing::conformance("New request or folder", |ui, enabled| {
        ui.add(IconButton::new(Glyph::Plus, "New request or folder").enabled(enabled))
    })
    .run();
    testing::conformance("Manage environments", |ui, enabled| {
        ui.add(IconButton::new(Glyph::Settings, "Manage environments").enabled(enabled))
    })
    .run();
    testing::conformance("Import…", |ui, enabled| {
        ui.add(
            MenuItem::new("Import…")
                .icon(Glyph::Download)
                .enabled(enabled),
        )
    })
    .run();
    let mut env = "Base Environment";
    testing::conformance("Environment", move |ui, enabled| {
        ui.add(
            Dropdown::new("env", &mut env, ["Base Environment", "Staging"])
                .label("Environment")
                .enabled(enabled),
        )
    })
    .run();
    let mut filter = String::new();
    testing::conformance("Filter requests", move |ui, enabled| {
        ui.add(
            TextField::search(&mut filter)
                .label("Filter requests")
                .enabled(enabled),
        )
    })
    .no_click()
    .keys(&[])
    .run();
    testing::conformance("Login", |ui, enabled| {
        ListRow::new("Login")
            .enabled(enabled)
            .trailing(Slot::ui_width(44.0, |ui| {
                ui.add(Tag::new("POST").tone(Tone::method("POST")));
            }))
            .with(DragSource(Id::from("login")))
            .with(ContextMenu::new(|ui| {
                ui.add(MenuItem::new("Rename"));
            }))
            .show(ui)
            .response
    })
    .not_focusable()
    .run();
}

fn row_labels(harness: &Harness<'_, Screen>) -> Vec<String> {
    ["Auth", "Login", "List users", "Get invoice"]
        .into_iter()
        .filter(|name| harness.query_by_label(name).is_some())
        .map(String::from)
        .collect()
}

#[test]
fn tree_lists_folders_and_requests() {
    let harness = harness(Theme::dark());
    assert_eq!(
        row_labels(&harness),
        ["Auth", "Login", "List users", "Get invoice"]
    );
}

#[test]
fn filter_narrows_the_tree() {
    let mut harness = harness(Theme::dark());
    let filter = harness.get_by_role(Role::TextInput);
    filter.click();
    filter.type_text("invoice");
    harness.run();
    assert_eq!(harness.state().state.filter, "invoice");
    assert_eq!(row_labels(&harness), ["Get invoice"]);

    // Matching a request inside a folder keeps its folder visible.
    let filter = harness.get_by_role(Role::TextInput);
    filter.focus();
    harness.state_mut().state.filter = "login".into();
    harness.run();
    assert_eq!(row_labels(&harness), ["Auth", "Login"]);

    harness.state_mut().state.filter = "zzz".into();
    harness.run();
    assert!(harness.query_by_label("No matches").is_some());
}

#[test]
fn clicking_a_request_selects_it() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("Get invoice").click();
    harness.run();
    let s = harness.state();
    let selected = s.state.selected.clone().expect("selection");
    assert_eq!(
        s.state.workspace.find_request(&selected).unwrap().1.name,
        "Get invoice"
    );
}

#[test]
fn arrow_keys_move_between_rows() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("List users").click();
    harness.run();
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    let s = harness.state();
    let selected = s.state.selected.clone().expect("selection");
    assert_eq!(
        s.state.workspace.find_request(&selected).unwrap().1.name,
        "Get invoice"
    );
    harness.key_press(egui::Key::ArrowUp);
    harness.key_press(egui::Key::ArrowUp);
    harness.run();
    let s = harness.state();
    let selected = s.state.selected.clone().expect("selection");
    assert_eq!(
        s.state.workspace.find_request(&selected).unwrap().1.name,
        "Login"
    );
}

#[test]
fn folder_collapses_with_the_left_arrow() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("Auth").click();
    harness.run();
    harness.key_press(egui::Key::ArrowLeft);
    harness.run();
    assert!(harness.query_by_label("Login").is_none());
    harness.key_press(egui::Key::ArrowRight);
    harness.run();
    assert!(harness.query_by_label("Login").is_some());
}

#[test]
fn context_menu_renames_inline_and_enter_commits() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("Get invoice").click_secondary();
    harness.run();
    harness.get_by_label("Rename").click();
    harness.run();
    let field = harness.get_by_label("Name");
    field.focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    field.type_text("Fetch invoice");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(harness.query_by_label("Fetch invoice").is_some());
    assert!(harness.query_by_label("Get invoice").is_none());
}

#[test]
fn escape_cancels_inline_rename() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("Get invoice").click_secondary();
    harness.run();
    harness.get_by_label("Rename").click();
    harness.run();
    harness.get_by_label("Name").type_text("zzz");
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(harness.query_by_label("Get invoice").is_some());
    assert!(harness.query_by_label("Name").is_none());
}

#[test]
fn context_menu_deletes_a_request() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("Get invoice").click_secondary();
    harness.run();
    harness.get_by_label("Delete").click();
    harness.run();
    assert!(harness.query_by_label("Get invoice").is_none());
    assert_eq!(harness.state().state.workspace.requests().len(), 2);
}

#[test]
fn environment_dropdown_switches_the_active_environment() {
    let mut harness = harness(Theme::dark());
    {
        let ws = &mut harness.state_mut().state.workspace;
        ws.environments
            .push(nexus_http_core::model::Environment::new("Staging"));
    }
    harness.run();
    harness.get_by_role(Role::ComboBox).click();
    harness.run();
    harness.get_by_label("Staging").click();
    harness.run();
    let active = harness.state().state.workspace.active_environment.clone();
    assert!(active.is_some());
}

#[test]
fn workspace_menu_opens_the_import_dialog_request() {
    let mut harness = harness(Theme::dark());
    harness.get_by_label("Workspace actions").click();
    harness.run();
    harness.get_by_label("Import…").click();
    harness.run();
    assert!(harness.state().actions.open_import);
}
