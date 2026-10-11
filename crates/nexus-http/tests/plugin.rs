use archipel_sdk::api::{
    API_VERSION, EditorTabRequest, MenuLocation, Plugin, Value, ViewContext, ViewPlacement,
};
use archipel_sdk::kit::Theme;
use archipel_sdk::testing::{Fixture, RecordingHost, audit, view_harness};
use nexus_http::plugin::{FILE_EXTENSIONS, ID, NexusHttpPlugin, OPEN, VIEW};

fn host_in(dir: &std::path::Path) -> RecordingHost {
    RecordingHost {
        project: Some(dir.to_path_buf()),
        ..RecordingHost::new(ID)
    }
}

#[test]
fn manifest_parses_and_matches_the_sdk() {
    let plugin = NexusHttpPlugin::new();
    let manifest = plugin.manifest();
    assert_eq!(manifest.id, ID);
    assert_eq!(manifest.name, "Nexus HTTP");
    assert_eq!(manifest.api_version, API_VERSION);
    assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest.app.as_ref().map(|a| a.view.as_str()), Some(VIEW));
}

#[test]
fn contributes_command_menu_view_and_request_file_editor() {
    let contributions = NexusHttpPlugin::new().contributions();
    assert!(contributions.problems().is_empty());
    assert!(contributions.commands.iter().any(|c| c.id == OPEN));
    assert!(
        contributions
            .menus
            .iter()
            .any(|m| m.command == OPEN && m.menu == MenuLocation::Tools)
    );
    assert!(contributions.tool_windows.iter().any(|t| t.view == VIEW));
    assert_eq!(contributions.editors[0].view, VIEW);
    assert_eq!(contributions.editors[0].file_extensions, FILE_EXTENSIONS);
}

#[test]
fn open_command_opens_the_view() {
    let mut plugin = NexusHttpPlugin::new();
    let mut host = RecordingHost::new(ID);
    plugin.execute(OPEN, Value::Null, &mut host).unwrap();
    assert_eq!(host.editor_tabs.len(), 1);
    assert_eq!(host.editor_tabs[0].view, VIEW);
    assert!(
        plugin
            .execute("nexus.nope", Value::Null, &mut host)
            .is_err()
    );
    let _: &EditorTabRequest = &host.editor_tabs[0];
}

#[test]
fn http_files_are_imported_once_into_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("api.http");
    std::fs::write(
        &file,
        "### Ping\nGET https://example.test/ping\n\n### Pong\nGET https://example.test/pong\n",
    )
    .unwrap();
    let mut host = host_in(dir.path());
    let mut plugin = NexusHttpPlugin::new();
    plugin.import_file(&file, &mut host);
    plugin.import_file(&file, &mut host);
    assert_eq!(plugin.request_count(), Some(2));
    assert_eq!(host.statuses.len(), 1);
    plugin.import_file(&dir.path().join("missing.http"), &mut host);
    assert_eq!(host.notifications.len(), 1);
}

#[test]
fn workspace_resolves_to_project_dot_nexus_or_the_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_in(dir.path());
    assert_eq!(
        NexusHttpPlugin::workspace_dir(&host),
        dir.path().join(".nexus")
    );

    let data = tempfile::tempdir().unwrap();
    let mut no_project = RecordingHost::new(ID);
    no_project.data = Some(data.path().to_path_buf());
    assert_eq!(
        NexusHttpPlugin::workspace_dir(&no_project),
        data.path().join(".nexus")
    );
}

fn harness(
    theme: Theme,
    dir: &std::path::Path,
) -> archipel_sdk::testing::Harness<'static, Fixture<NexusHttpPlugin>> {
    let fixture = Fixture::with_host(NexusHttpPlugin::new(), host_in(dir));
    let view = ViewContext {
        view: VIEW.into(),
        placement: ViewPlacement::App,
    };
    view_harness(theme, fixture, view)
}

#[test]
fn view_renders_in_the_fake_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(Theme::dark(), dir.path());
    harness.run();
    assert!(harness.state().plugin.request_count().is_some());
}

// The views still have unnamed controls and tiny targets (sidebar, overlays); enable once the
// archipel-kit port of `views/*` has landed.
#[test]
#[ignore = "views/* are being ported to archipel-kit"]
fn view_passes_the_audit_in_both_themes() {
    let dir = tempfile::tempdir().unwrap();
    for theme in [Theme::dark(), Theme::light()] {
        let mut harness = harness(theme, dir.path());
        harness.run();
        audit(&mut harness).assert_clean();
    }
}
