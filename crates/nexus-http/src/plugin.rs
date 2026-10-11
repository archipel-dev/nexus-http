//! Archipel plugin entry point: the HTTP client as an editor tab or tool window
//! (Tools › HTTP Client). `.http` and `.rest` files open in the same view and are imported
//! into the workspace. The views stay host-agnostic: [`NexusApp`] is also the standalone app.

use crate::NexusApp;
use archipel_sdk::api::{
    API_VERSION, CommandContribution, CommandError, CommandResult, Contributions,
    EditorContribution, EditorTabRequest, MenuLocation, Notification, Plugin, PluginHost,
    PluginManifest, ToolWindowContribution, Value, ViewContext, ViewFrame, ViewPlacement,
};
use archipel_sdk::ui;
use nexus_http_core::import::http_file;
use nexus_http_core::storage::DIR_NAME;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const ID: &str = "dev.archipel.nexus-http";
pub const VIEW: &str = "nexus.main";
pub const OPEN: &str = "nexus.open";
pub const TOOL_WINDOW: &str = "nexus";
const TITLE: &str = "HTTP Client";
/// Extensions of request files the editor opens.
pub const FILE_EXTENSIONS: [&str; 2] = ["http", "rest"];

const MANIFEST: &str = include_str!("../archipel-plugin.toml");

/// The bundled manifest, with its version taken from this crate's `CARGO_PKG_VERSION`.
pub fn manifest() -> PluginManifest {
    let manifest = PluginManifest::parse(&MANIFEST.replace("@VERSION@", env!("CARGO_PKG_VERSION")))
        .expect("bundled manifest is valid");
    debug_assert_eq!(manifest.api_version, API_VERSION);
    manifest
}

pub struct NexusHttpPlugin {
    manifest: PluginManifest,
    /// Created on first render, once the project and data directories are known.
    app: Option<NexusApp>,
    imported: HashSet<PathBuf>,
}

impl Default for NexusHttpPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl NexusHttpPlugin {
    /// The constructor a host registers as the plugin factory.
    pub fn new() -> Self {
        Self {
            manifest: manifest(),
            app: None,
            imported: HashSet::new(),
        }
    }

    /// Directory that holds the workspace: the project root, or the plugin data directory
    /// when no project is open.
    pub fn base_dir(host: &dyn PluginHost) -> PathBuf {
        host.project_root()
            .map(Path::to_path_buf)
            .or_else(|| host.data_dir().ok())
            .unwrap_or_else(|| std::env::temp_dir().join("nexus-http"))
    }

    /// The workspace lives in `<project>/.nexus` (so it can be committed).
    pub fn workspace_dir(host: &dyn PluginHost) -> PathBuf {
        Self::base_dir(host).join(DIR_NAME)
    }

    fn app(&mut self, host: &dyn PluginHost) -> &mut NexusApp {
        self.app
            .get_or_insert_with(|| NexusApp::open(&Self::base_dir(host)))
    }

    /// Number of requests in the open workspace, `None` before the first render.
    pub fn request_count(&self) -> Option<usize> {
        self.app
            .as_ref()
            .map(|app| app.state.workspace.requests().len())
    }

    /// Imports a `.http`/`.rest` file into the workspace, once per path.
    pub fn import_file(&mut self, path: &Path, host: &mut dyn PluginHost) {
        if !self.imported.insert(path.to_path_buf()) {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let result = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| http_file::import(&text, &name).map_err(|e| e.to_string()));
        match result {
            Ok(imported) => {
                let count = imported.workspace.requests().len();
                self.app(host).state.merge_import(imported.workspace);
                host.show_status(&format!("Imported {count} request(s) from {name}"));
            }
            Err(error) => host.notify(Notification::error(
                "Cannot open request file",
                format!("{name}: {error}"),
            )),
        }
    }
}

impl Plugin for NexusHttpPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn contributions(&self) -> Contributions {
        let mut editor = EditorContribution::new(VIEW, TITLE);
        for extension in FILE_EXTENSIONS {
            editor = editor.file_extension(extension);
        }
        Contributions::new()
            .command(CommandContribution::new(OPEN, TITLE))
            .menu(MenuLocation::Tools, OPEN)
            .tool_window(ToolWindowContribution::new(TOOL_WINDOW, TITLE, VIEW).icon_text("N"))
            .editor(editor)
    }

    fn execute(
        &mut self,
        command: &str,
        _params: Value,
        host: &mut dyn PluginHost,
    ) -> CommandResult {
        match command {
            OPEN => {
                host.open_editor_tab(EditorTabRequest::new(VIEW, TITLE).key(VIEW));
                Ok(Value::Null)
            }
            other => Err(CommandError::Unknown(other.into())),
        }
    }

    fn render_view(
        &mut self,
        view: &ViewContext,
        frame: &mut ViewFrame<'_>,
        host: &mut dyn PluginHost,
    ) {
        let Some(ui) = ui::ui(frame) else {
            return;
        };
        if view.view != VIEW {
            ui::empty_state(ui, "Unknown view", &view.view);
            return;
        }
        if let ViewPlacement::EditorTab {
            path: Some(path), ..
        } = &view.placement
        {
            self.import_file(path, host);
        }
        self.app(host).ui(ui);
    }

    fn poll(&mut self, host: &mut dyn PluginHost) {
        if let Some(app) = &mut self.app
            && app.poll()
        {
            host.request_repaint();
        }
    }
}
