//! Collection sidebar: workspace header, environment picker, filter, and the
//! folder/request tree with context menus, inline rename and drag & drop, all on
//! archipel-kit.

use crate::state::NexusState;
use archipel_kit::behaviour::{Behaviour, ContextMenu, DragSource, DropTarget};
use archipel_kit::components::{
    Avatar, Dropdown, EmptyState, IconButton, ListRow, MenuItem, MenuSeparator, Tag, TextField,
    VirtualList,
};
use archipel_kit::parts::{Popover, Slot};
use archipel_kit::tokens::{size, typography};
use archipel_kit::{Glyph, Theme, Tone, cursor, egui};
use egui::{Key, Modifiers, Rect, Response, Ui, pos2, vec2};
use nexus_http_core::model::{Id, Item, Protocol, Request};
use std::cell::{Cell, RefCell};
use std::fmt;

#[derive(Default)]
pub struct SidebarUi {
    /// The row being renamed: (item, edit buffer, focus the field on the next frame).
    renaming: Option<(Id, String, bool)>,
    /// The row the keyboard cursor is on (requests and folders).
    cursor: Option<Id>,
    /// The request selected in the state when `cursor` was last synced.
    last_selected: Option<Id>,
    workspace_menu: bool,
    add_menu: bool,
}

#[derive(Default)]
pub struct SidebarActions {
    pub open_import: bool,
    pub open_environments: bool,
    pub export: bool,
    /// Open the collection runner for a folder (`Some(None)` = whole workspace).
    pub run: Option<Option<Id>>,
}

/// Where a dragged item lands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Drop {
    After(Id),
    Into(Id),
    /// End of the top level.
    End,
}

/// A row of the flattened, visible tree.
struct Row {
    id: Id,
    name: String,
    depth: usize,
    /// `Some(open)` for folders.
    folder: Option<bool>,
    /// Method (or protocol) tag of a request.
    tag: Option<(String, Tone)>,
}

/// What the rows report while they draw; applied after the list is done.
#[derive(Default)]
struct Sink {
    menu: Option<(Id, MenuAction)>,
    drop: Option<(Id, Drop)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    NewRequest,
    NewFolder,
    RunFolder,
    Send,
    Rename,
    Duplicate,
    Delete,
}

/// The environment dropdown's options.
#[derive(Clone, PartialEq)]
struct EnvChoice {
    id: Option<Id>,
    name: String,
}

impl fmt::Display for EnvChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// A [`DropTarget`] that reports into a shared sink as a [`Drop`] at `target`.
struct DropOn<'a>(&'a RefCell<Sink>, Drop);

impl Behaviour for DropOn<'_> {
    fn attach(self, ui: &mut Ui, response: Response) -> Response {
        let mut dropped: Option<Id> = None;
        let response = DropTarget::new(&mut dropped).attach(ui, response);
        if let Some(dragged) = dropped {
            self.0.borrow_mut().drop = Some((dragged, self.1));
        }
        response
    }
}

pub fn show(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut SidebarUi,
    actions: &mut SidebarActions,
) {
    let colors = Theme::colors(ui.ctx());
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        ui.add(Avatar::new(state.workspace.name.clone()).size(20.0));
        ui.label(
            egui::RichText::new(&state.workspace.name)
                .font(typography::semibold(ui.ctx(), 14.0))
                .color(colors.text_strong),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let more = ui.add(IconButton::new(Glyph::More, "Workspace actions"));
            if more.clicked() {
                local.workspace_menu = !local.workspace_menu;
            }
            let mut menu = local.workspace_menu;
            let closed = Popover::below("nexus-workspace-menu", &more)
                .width(210.0)
                .show(ui.ctx(), &mut menu, |ui| {
                    let mut chosen = false;
                    if ui
                        .add(MenuItem::new("Import…").icon(Glyph::Download))
                        .clicked()
                    {
                        actions.open_import = true;
                        chosen = true;
                    }
                    if ui
                        .add(MenuItem::new("Run collection…").icon(Glyph::Run))
                        .clicked()
                    {
                        actions.run = Some(None);
                        chosen = true;
                    }
                    if ui
                        .add(MenuItem::new("Export workspace…").icon(Glyph::Upload))
                        .clicked()
                    {
                        actions.export = true;
                        chosen = true;
                    }
                    ui.add(MenuSeparator);
                    if ui
                        .add(MenuItem::new("Manage environments…").icon(Glyph::Settings))
                        .clicked()
                    {
                        actions.open_environments = true;
                        chosen = true;
                    }
                    chosen
                });
            local.workspace_menu = menu && !closed.unwrap_or(false);
        });
    });
    ui.add_space(6.0);
    environment_picker(ui, state, actions);
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        let width = ui.available_width() - size::CONTROL_HEIGHT - 12.0;
        let filter = ui.add(
            TextField::search(&mut state.filter)
                .placeholder("Filter requests")
                .width(width),
        );
        // The text edit gives up the focus on Escape, so look at the key it just saw.
        if (filter.has_focus() || filter.lost_focus())
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            state.filter.clear();
        }
        let plus = ui.add(IconButton::new(Glyph::Plus, "New request or folder"));
        if plus.clicked() {
            local.add_menu = !local.add_menu;
        }
        let mut menu = local.add_menu;
        let closed =
            Popover::below("nexus-add-menu", &plus)
                .width(190.0)
                .show(ui.ctx(), &mut menu, |ui| {
                    let mut chosen = false;
                    if ui
                        .add(MenuItem::new("New Request").icon(Glyph::NewFile))
                        .clicked()
                    {
                        state.new_request(None);
                        chosen = true;
                    }
                    if ui
                        .add(MenuItem::new("New Folder").icon(Glyph::NewFolder))
                        .clicked()
                    {
                        let id = state.new_folder(None);
                        local.renaming = Some((id, "New Folder".into(), true));
                        chosen = true;
                    }
                    ui.add(MenuSeparator);
                    if ui
                        .add(MenuItem::new("Import…").icon(Glyph::Download))
                        .clicked()
                    {
                        actions.open_import = true;
                        chosen = true;
                    }
                    chosen
                });
        local.add_menu = menu && !closed.unwrap_or(false);
    });
    ui.add_space(6.0);

    tree(ui, state, local, actions);
}

fn environment_picker(ui: &mut Ui, state: &mut NexusState, actions: &mut SidebarActions) {
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        let base = EnvChoice {
            id: None,
            name: "Base Environment".into(),
        };
        let mut options = vec![base.clone()];
        options.extend(state.workspace.environments.iter().map(|e| EnvChoice {
            id: Some(e.id.clone()),
            name: e.name.clone(),
        }));
        let mut choice = state
            .workspace
            .active_environment
            .as_ref()
            .and_then(|active| options.iter().find(|o| o.id.as_ref() == Some(active)))
            .cloned()
            .unwrap_or(base);
        let before = choice.id.clone();
        let width = ui.available_width() - size::CONTROL_HEIGHT - 12.0;
        ui.add(
            Dropdown::new("nexus-environment", &mut choice, options)
                .label("Environment")
                .width(width),
        );
        if choice.id != before {
            state.workspace.active_environment = choice.id;
            state.touch();
        }
        if ui
            .add(IconButton::new(Glyph::Settings, "Manage environments"))
            .clicked()
        {
            actions.open_environments = true;
        }
    });
}

fn matches(item: &Item, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    match item {
        Item::Request(r) => {
            r.name.to_lowercase().contains(filter) || r.url.to_lowercase().contains(filter)
        }
        Item::Folder(f) => {
            f.name.to_lowercase().contains(filter) || f.children.iter().any(|c| matches(c, filter))
        }
    }
}

fn request_tag(request: &Request) -> (String, Tone) {
    match request.protocol {
        Protocol::Http => {
            let method = request.method.to_string();
            let tone = Tone::method(&method);
            (method, tone)
        }
        Protocol::WebSocket => ("WS".into(), Tone::Accent),
        Protocol::Sse => ("SSE".into(), Tone::Link),
        Protocol::Grpc => ("gRPC".into(), Tone::Success),
    }
}

/// Flattens the visible part of the tree (expanded folders, or everything matching a filter).
fn flatten(state: &NexusState, items: &[Item], filter: &str, depth: usize, rows: &mut Vec<Row>) {
    for item in items.iter().filter(|i| matches(i, filter)) {
        match item {
            Item::Folder(f) => {
                let open = !filter.is_empty() || !state.collapsed.contains(&f.id);
                rows.push(Row {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    depth,
                    folder: Some(open),
                    tag: None,
                });
                if open {
                    flatten(state, &f.children, filter, depth + 1, rows);
                }
            }
            Item::Request(r) => rows.push(Row {
                id: r.id.clone(),
                name: r.name.clone(),
                depth,
                folder: None,
                tag: Some(request_tag(r)),
            }),
        }
    }
}

fn tree(ui: &mut Ui, state: &mut NexusState, local: &mut SidebarUi, actions: &mut SidebarActions) {
    if state.selected != local.last_selected {
        local.last_selected = state.selected.clone();
        local.cursor = state.selected.clone();
    }
    if state.workspace.items.is_empty() {
        if EmptyState::new("No requests yet")
            .icon(Glyph::Send)
            .hint("Press Ctrl+N or use + to add one.")
            .action("New request")
            .show(ui)
            .clicked()
        {
            state.new_request(None);
        }
        return;
    }
    let filter = state.filter.trim().to_lowercase();
    let mut rows = Vec::new();
    flatten(state, &state.workspace.items, &filter, 0, &mut rows);
    if rows.is_empty() {
        EmptyState::new("No matches")
            .icon(Glyph::Search)
            .hint("Nothing in this workspace matches the filter.")
            .show(ui);
        return;
    }

    let dragging = egui::DragAndDrop::has_payload_of_type::<Id>(ui.ctx());
    let end_zone = if dragging {
        size::ROW_HEIGHT + 6.0
    } else {
        0.0
    };
    let height = (ui.available_height() - end_zone).max(size::ROW_HEIGHT * 3.0);
    let mut selected = local
        .cursor
        .as_ref()
        .and_then(|id| rows.iter().position(|r| &r.id == id));

    let sink = RefCell::new(Sink::default());
    let rename_rect: Cell<Option<Rect>> = Cell::new(None);
    let renaming_id = local.renaming.as_ref().map(|(id, _, _)| id.clone());
    let out = VirtualList::new("nexus-tree", rows.len(), &mut selected)
        .name("Requests")
        .height(height)
        .show(ui, |i| {
            let row = &rows[i];
            let id = row.id.clone();
            let mut list_row = ListRow::new(row.name.as_str())
                .depth(row.depth)
                .expanded(row.folder);
            match row.folder {
                Some(open) => {
                    list_row = list_row.icon(if open {
                        Glyph::FolderOpen
                    } else {
                        Glyph::Folder
                    });
                }
                None => {
                    if let Some((text, tone)) = &row.tag {
                        // TODO(kit Tone::method): the tag sits in the trailing slot because
                        // ListRow has no leading slot (see "Kit gaps" in the PR).
                        let text = text.clone();
                        let tone = *tone;
                        list_row = list_row.trailing(Slot::ui_width(44.0, move |ui| {
                            ui.add(Tag::new(text).tone(tone));
                        }));
                    }
                }
            }
            let is_folder = row.folder.is_some();
            let menu_id = id.clone();
            let sink_ref = &sink;
            let rect_cell = &rename_rect;
            let is_renaming = renaming_id.as_ref() == Some(&id);
            let target = if is_folder {
                Drop::Into(id.clone())
            } else {
                Drop::After(id.clone())
            };
            list_row
                .with(move |_: &mut Ui, response: Response| {
                    if is_renaming {
                        rect_cell.set(Some(response.rect));
                    }
                    response
                })
                .with(ContextMenu::new(move |ui| {
                    let pick = |ui: &mut Ui, item: MenuItem, action: MenuAction| {
                        if ui.add(item).clicked() {
                            sink_ref.borrow_mut().menu = Some((menu_id.clone(), action));
                            ui.close();
                        }
                    };
                    if is_folder {
                        pick(
                            ui,
                            MenuItem::new("New Request").icon(Glyph::NewFile),
                            MenuAction::NewRequest,
                        );
                        pick(
                            ui,
                            MenuItem::new("New Folder").icon(Glyph::NewFolder),
                            MenuAction::NewFolder,
                        );
                        pick(
                            ui,
                            MenuItem::new("Run folder…").icon(Glyph::Run),
                            MenuAction::RunFolder,
                        );
                        ui.add(MenuSeparator);
                    } else {
                        pick(
                            ui,
                            MenuItem::new("Send").icon(Glyph::Send),
                            MenuAction::Send,
                        );
                    }
                    pick(
                        ui,
                        MenuItem::new("Rename").icon(Glyph::Edit).shortcut("F2"),
                        MenuAction::Rename,
                    );
                    pick(
                        ui,
                        MenuItem::new("Duplicate").icon(Glyph::Copy),
                        MenuAction::Duplicate,
                    );
                    ui.add(MenuSeparator);
                    pick(
                        ui,
                        MenuItem::new("Delete").icon(Glyph::Delete),
                        MenuAction::Delete,
                    );
                }))
                .with(DragSource(id))
                .with(DropOn(sink_ref, target))
                .with(|_: &mut Ui, response: Response| {
                    // Rows are not links: the arrow stays while hovering, even though the
                    // drag source offers the hand.
                    if !response.dragged() {
                        cursor::on_hover(&response, egui::CursorIcon::Default);
                    }
                    response
                })
        });

    if dragging {
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), size::ROW_HEIGHT + 6.0),
            egui::Sense::hover(),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.add_space(3.0);
            ui.add_enabled(
                false,
                egui::Label::new(
                    egui::RichText::new("Drop here to move to the top level")
                        .color(Theme::colors(ui.ctx()).text_secondary),
                ),
            );
        });
        DropOn(&sink, Drop::End).attach(ui, response);
    }

    // Keyboard cursor follows the list's selection; requests open in the editor.
    if out.selection_changed
        && let Some(row) = selected.and_then(|i| rows.get(i))
    {
        local.cursor = Some(row.id.clone());
        if row.folder.is_none() {
            state.select(row.id.clone());
            local.last_selected = state.selected.clone();
        }
    }
    if let Some(index) = out.toggled.or(out.activated)
        && let Some(row) = rows.get(index)
        && row.folder.is_some()
    {
        toggle_folder(state, &row.id);
    }
    // Left/Right on a focused tree collapse and expand the folder under the cursor.
    if out.response.has_focus()
        && filter.is_empty()
        && let Some(row) = selected.and_then(|i| rows.get(i))
    {
        let (left, right, rename) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::F2),
            )
        });
        match row.folder {
            Some(true) if left => toggle_folder(state, &row.id),
            Some(false) if right => toggle_folder(state, &row.id),
            _ => {}
        }
        if rename {
            local.renaming = Some((row.id.clone(), row.name.clone(), true));
        }
    }

    inline_rename(ui, state, local, &rows, rename_rect.get());

    let Sink { menu, drop } = sink.into_inner();
    if let Some((dragged, target)) = drop {
        apply_drop(state, &dragged, target);
    }
    if let Some((id, action)) = menu {
        run_menu_action(state, local, actions, &id, action);
    }
}

fn toggle_folder(state: &mut NexusState, id: &Id) {
    if !state.collapsed.remove(id) {
        state.collapsed.insert(id.clone());
    }
}

/// A text field over the row being renamed: Enter or leaving the field commits, Escape cancels.
fn inline_rename(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut SidebarUi,
    rows: &[Row],
    rect: Option<Rect>,
) {
    let Some((id, buffer, focus)) = local.renaming.as_mut() else {
        return;
    };
    let Some(row) = rows.iter().find(|r| &r.id == id) else {
        local.renaming = None;
        return;
    };
    let Some(rect) = rect else {
        // Scrolled out of view (the list only lays out visible rows): cancel.
        local.renaming = None;
        return;
    };
    let left = rect.left() + size::indent(row.depth) + size::ICON + 8.0;
    let field = Rect::from_min_max(
        pos2(left, rect.top()),
        pos2(rect.right() - 4.0, rect.bottom()),
    );
    let escape = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let edit = ui.put(
        field,
        TextField::new(buffer).label("Name").width(field.width()),
    );
    if *focus {
        edit.request_focus();
        *focus = false;
    }
    let enter = edit.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if escape {
        local.renaming = None;
    } else if enter || edit.lost_focus() {
        let name = buffer.clone();
        let id = id.clone();
        local.renaming = None;
        state.rename(&id, &name);
    }
}

fn run_menu_action(
    state: &mut NexusState,
    local: &mut SidebarUi,
    actions: &mut SidebarActions,
    id: &Id,
    action: MenuAction,
) {
    let is_folder = matches!(state.workspace.find_item(id), Some(Item::Folder(_)));
    let name = state
        .workspace
        .find_item(id)
        .map(|item| item.name().to_owned())
        .unwrap_or_default();
    match action {
        MenuAction::NewRequest => {
            state.new_request(Some(id));
        }
        MenuAction::NewFolder => {
            let new = state.new_folder(Some(id));
            local.renaming = Some((new, "New Folder".into(), true));
        }
        MenuAction::RunFolder => actions.run = Some(Some(id.clone())),
        MenuAction::Send => {
            state.select(id.clone());
            state.send(id);
        }
        MenuAction::Rename => local.renaming = Some((id.clone(), name, true)),
        MenuAction::Duplicate => {
            if let Some(copy) = state.duplicate(id)
                && !is_folder
            {
                state.select(copy);
            }
        }
        MenuAction::Delete => state.delete(id),
    }
}

fn apply_drop(state: &mut NexusState, dragged: &Id, target: Drop) {
    let locate = |state: &NexusState, id: &Id| -> Option<(Option<Id>, usize)> {
        fn walk(items: &[Item], parent: Option<&Id>, id: &Id) -> Option<(Option<Id>, usize)> {
            for (i, item) in items.iter().enumerate() {
                if item.id() == id {
                    return Some((parent.cloned(), i));
                }
                if let Item::Folder(f) = item
                    && let Some(found) = walk(&f.children, Some(&f.id), id)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&state.workspace.items, None, id)
    };
    match target {
        Drop::Into(folder) => {
            if &folder == dragged {
                return;
            }
            let end = match state.workspace.find_item(&folder) {
                Some(Item::Folder(f)) => f.children.len(),
                _ => return,
            };
            state.move_item(dragged, Some(&folder), end);
        }
        Drop::End => {
            let len = state.workspace.items.len();
            state.move_item(dragged, None, len);
        }
        Drop::After(t) => {
            if &t == dragged {
                return;
            }
            if let Some((parent, index)) = locate(state, &t) {
                state.move_item(dragged, parent.as_ref(), index + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_http_core::model::Workspace;

    #[test]
    fn drops_reorder_and_nest() {
        let mut s = NexusState::in_memory(Workspace::new("W"));
        let a = s.new_request(None);
        let b = s.new_request(None);
        let f = s.new_folder(None);
        apply_drop(&mut s, &b, Drop::After(f.clone()));
        assert_eq!(s.workspace.items.last().unwrap().id(), &b);
        apply_drop(&mut s, &a, Drop::Into(f.clone()));
        let Some(Item::Folder(folder)) = s.workspace.find_item(&f) else {
            panic!()
        };
        assert_eq!(folder.children[0].id(), &a);
        apply_drop(&mut s, &a, Drop::End);
        assert_eq!(s.workspace.items.last().unwrap().id(), &a);
    }
}
