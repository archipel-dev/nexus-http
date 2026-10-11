//! Collection sidebar: workspace header, environment picker, filter, and the
//! folder/request tree with context menus, inline rename and drag & drop.

use crate::state::NexusState;
use crate::theme::{Palette, semibold};
use crate::widgets::{self, Icon, icon_button, paint_icon};
use egui::{Align2, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use nexus_http_core::model::{Id, Item};

const ROW_H: f32 = 28.0;
const INDENT: f32 = 14.0;

#[derive(Default)]
pub struct SidebarUi {
    renaming: Option<(Id, String, bool)>,
    run_request: Option<Id>,
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
    Before(Id),
    After(Id),
    Into(Id),
    /// End of the top level.
    End,
}

pub fn show(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut SidebarUi,
    actions: &mut SidebarActions,
) {
    let pal = Palette::of(ui);
    ui.add_space(10.0);
    // Workspace header
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let name = state.workspace.name.clone();
        ui.label(
            RichText::new(name)
                .font(semibold(ui, 14.0))
                .color(pal.strong),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let more = icon_button(ui, Icon::Dots, pal.weak, "Workspace actions");
            egui::Popup::menu(&more).width(200.0).show(|ui| {
                if ui.button("Import…").clicked() {
                    actions.open_import = true;
                }
                if ui.button("Run collection…").clicked() {
                    actions.run = Some(None);
                }
                if ui.button("Export workspace…").clicked() {
                    actions.export = true;
                }
                if ui.button("Manage environments…").clicked() {
                    actions.open_environments = true;
                }
            });
        });
    });
    ui.add_space(6.0);
    environment_picker(ui, state, &pal, actions);
    ui.add_space(8.0);

    // Filter + new
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let w = ui.available_width() - 44.0;
        widgets::field_frame(&pal)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.set_width(w - 14.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
                    paint_icon(ui.painter(), r, Icon::Search, pal.faint);
                    ui.add(
                        egui::TextEdit::singleline(&mut state.filter)
                            .frame(false)
                            .hint_text("Filter")
                            .desired_width(ui.available_width()),
                    );
                });
            });
        let plus = icon_button(ui, Icon::Plus, pal.weak, "New request or folder");
        egui::Popup::menu(&plus).width(180.0).show(|ui| {
            if ui.button("New Request").clicked() {
                state.new_request(None);
            }
            if ui.button("New Folder").clicked() {
                let id = state.new_folder(None);
                local.renaming = Some((id, "New Folder".into(), true));
            }
            ui.separator();
            if ui.button("Import…").clicked() {
                actions.open_import = true;
            }
        });
    });
    ui.add_space(6.0);

    let filter = state.filter.trim().to_lowercase();
    let items = state.workspace.items.clone();
    let mut drop: Option<(Id, Drop)> = None;
    egui::ScrollArea::vertical()
        .id_salt("nexus-tree")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if items.is_empty() {
                widgets::empty_state(ui, "No requests yet", "Press Ctrl+N or use + to add one.");
                return;
            }
            for item in &items {
                tree_item(ui, state, local, item, 0, &filter, &pal, &mut drop);
            }
            // Space below the tree accepts drops to the end of the top level.
            let (rest, resp) = ui.allocate_exact_size(
                vec2(ui.available_width(), ui.available_height().max(24.0)),
                Sense::hover(),
            );
            if resp.dnd_hover_payload::<Id>().is_some() {
                ui.painter().hline(
                    rest.x_range(),
                    rest.top() + 1.0,
                    Stroke::new(2.0_f32, pal.accent),
                );
                if let Some(released) = resp.dnd_release_payload::<Id>() {
                    drop = Some(((*released).clone(), Drop::End));
                }
            }
        });

    if let Some((dragged, target)) = drop {
        apply_drop(state, &dragged, target);
    }
    if let Some(folder) = local.run_request.take() {
        actions.run = Some(Some(folder));
    }
}

fn environment_picker(
    ui: &mut Ui,
    state: &mut NexusState,
    pal: &Palette,
    actions: &mut SidebarActions,
) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let active = state.workspace.active_environment().cloned();
        let (name, color) = match &active {
            Some(env) => (
                env.name.clone(),
                env.color
                    .as_deref()
                    .and_then(widgets::parse_color)
                    .unwrap_or(pal.accent),
            ),
            None => ("Base Environment".to_owned(), pal.faint),
        };
        let w = ui.available_width() - 10.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
        let fill = if resp.hovered() {
            pal.hover
        } else {
            color.gamma_multiply(0.10)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            fill,
            Stroke::new(1.0_f32, color.gamma_multiply(0.35)),
            egui::StrokeKind::Inside,
        );
        ui.painter()
            .circle_filled(pos2(rect.left() + 14.0, rect.center().y), 4.0, color);
        ui.painter().text(
            pos2(rect.left() + 26.0, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(13.0),
            pal.text,
        );
        paint_icon(
            ui.painter(),
            Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(10.0, 10.0)),
            Icon::Chevron(true),
            pal.weak,
        );
        let resp = resp.on_hover_text("Switch environment (Ctrl+E to edit)");
        egui::Popup::menu(&resp).width(w).show(|ui| {
            if ui
                .selectable_label(active.is_none(), "No environment (base only)")
                .clicked()
            {
                state.workspace.active_environment = None;
                state.touch();
            }
            let envs = state.workspace.environments.clone();
            for env in envs {
                let c = env
                    .color
                    .as_deref()
                    .and_then(widgets::parse_color)
                    .unwrap_or(pal.accent);
                let selected = active.as_ref().is_some_and(|a| a.id == env.id);
                ui.horizontal(|ui| {
                    widgets::dot(ui, c);
                    if ui.selectable_label(selected, &env.name).clicked() {
                        state.workspace.active_environment = Some(env.id.clone());
                        state.touch();
                        ui.close();
                    }
                });
            }
            ui.separator();
            if ui.button("Manage environments…").clicked() {
                actions.open_environments = true;
            }
        });
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

#[allow(clippy::too_many_arguments)]
fn tree_item(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut SidebarUi,
    item: &Item,
    depth: usize,
    filter: &str,
    pal: &Palette,
    drop: &mut Option<(Id, Drop)>,
) {
    if !matches(item, filter) {
        return;
    }
    let id = item.id().clone();
    let is_folder = matches!(item, Item::Folder(_));
    let open = is_folder && (!filter.is_empty() || !state.collapsed.contains(&id));
    let selected = state.selected.as_ref() == Some(&id);
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click_and_drag());
    let row = rect.shrink2(vec2(6.0, 1.0));
    if selected {
        ui.painter()
            .rect_filled(row, CornerRadius::same(6), pal.selection);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(row, CornerRadius::same(6), pal.hover);
    }
    let x = row.left() + 8.0 + depth as f32 * INDENT;
    let cy = row.center().y;
    let renaming = local.renaming.as_ref().is_some_and(|(r, _, _)| r == &id);

    match item {
        Item::Folder(f) => {
            paint_icon(
                ui.painter(),
                Rect::from_center_size(pos2(x + 5.0, cy), vec2(10.0, 10.0)),
                Icon::Chevron(open),
                pal.weak,
            );
            paint_icon(
                ui.painter(),
                Rect::from_center_size(pos2(x + 22.0, cy), vec2(14.0, 14.0)),
                Icon::Folder(open),
                pal.weak,
            );
            if !renaming {
                ui.painter().text(
                    pos2(x + 36.0, cy),
                    Align2::LEFT_CENTER,
                    &f.name,
                    FontId::proportional(13.0),
                    pal.text,
                );
            }
        }
        Item::Request(r) => {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(x - 6.0, cy - 8.0),
                vec2(40.0, 16.0),
            )));
            widgets::request_label(&mut child, r, 36.0);
            if !renaming {
                let max_w = row.right() - (x + 40.0) - 6.0;
                let name = elide(ui, &r.name, max_w);
                ui.painter().text(
                    pos2(x + 40.0, cy),
                    Align2::LEFT_CENTER,
                    name,
                    FontId::proportional(13.0),
                    if selected { pal.strong } else { pal.text },
                );
            }
        }
    }

    if let Some((_, buffer, focus)) = local.renaming.as_mut().filter(|(r, _, _)| r == &id) {
        let edit_rect = Rect::from_min_max(
            pos2(x + if is_folder { 32.0 } else { 36.0 }, cy - 10.0),
            pos2(row.right() - 4.0, cy + 10.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect));
        let edit = child.add(egui::TextEdit::singleline(buffer).desired_width(edit_rect.width()));
        if *focus {
            edit.request_focus();
            *focus = false;
        }
        let enter = child.input(|i| i.key_pressed(egui::Key::Enter));
        let escape = child.input(|i| i.key_pressed(egui::Key::Escape));
        if escape {
            local.renaming = None;
        } else if edit.lost_focus() || enter {
            let name = buffer.clone();
            local.renaming = None;
            state.rename(&id, &name);
        }
    }

    // Interaction
    if resp.clicked() {
        if is_folder {
            if !state.collapsed.remove(&id) {
                state.collapsed.insert(id.clone());
            }
        } else {
            state.select(id.clone());
        }
    }
    if resp.double_clicked() {
        local.renaming = Some((id.clone(), item.name().to_owned(), true));
    }
    resp.dnd_set_drag_payload(id.clone());
    if let Some(payload) = resp.dnd_hover_payload::<Id>()
        && *payload != id
    {
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
        let rel = (pointer.y - rect.top()) / rect.height();
        let target = if is_folder && (0.3..0.7).contains(&rel) {
            Drop::Into(id.clone())
        } else if rel < 0.5 {
            Drop::Before(id.clone())
        } else {
            Drop::After(id.clone())
        };
        match &target {
            Drop::Into(_) => {
                ui.painter().rect_stroke(
                    row,
                    CornerRadius::same(6),
                    Stroke::new(1.5_f32, pal.accent),
                    egui::StrokeKind::Inside,
                );
            }
            Drop::Before(_) => {
                ui.painter().hline(
                    row.x_range(),
                    rect.top() + 1.0,
                    Stroke::new(2.0_f32, pal.accent),
                );
            }
            Drop::After(_) => {
                ui.painter().hline(
                    row.x_range(),
                    rect.bottom() - 1.0,
                    Stroke::new(2.0_f32, pal.accent),
                );
            }
            Drop::End => {}
        }
        if let Some(released) = resp.dnd_release_payload::<Id>() {
            *drop = Some(((*released).clone(), target));
        }
    }
    resp.context_menu(|ui| {
        ui.set_min_width(180.0);
        if is_folder {
            if ui.button("New Request").clicked() {
                state.new_request(Some(&id));
            }
            if ui.button("New Folder").clicked() {
                let new = state.new_folder(Some(&id));
                local.renaming = Some((new, "New Folder".into(), true));
            }
            if ui.button("Run folder…").clicked() {
                local.run_request = Some(id.clone());
            }
            ui.separator();
        } else if ui.button("Send").clicked() {
            state.select(id.clone());
            state.send(&id);
        }
        if ui.button("Rename").clicked() {
            local.renaming = Some((id.clone(), item.name().to_owned(), true));
        }
        if ui.button("Duplicate").clicked()
            && let Some(copy) = state.duplicate(&id)
            && !is_folder
        {
            state.select(copy);
        }
        ui.separator();
        if ui
            .button(RichText::new("Delete").color(pal.error))
            .clicked()
        {
            state.delete(&id);
        }
    });

    if let Item::Folder(f) = item
        && open
    {
        for child in &f.children {
            tree_item(ui, state, local, child, depth + 1, filter, pal, drop);
        }
    }
}

fn elide(ui: &Ui, text: &str, max_w: f32) -> String {
    let font = FontId::proportional(13.0);
    let width = |s: &str| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    if width(text) <= max_w {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if width(&candidate) <= max_w {
            return candidate;
        }
    }
    "…".into()
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
        Drop::Before(t) => {
            if let Some((parent, index)) = locate(state, &t) {
                state.move_item(dragged, parent.as_ref(), index);
            }
        }
        Drop::After(t) => {
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
        apply_drop(&mut s, &b, Drop::Before(a.clone()));
        assert_eq!(s.workspace.items[0].id(), &b);
        apply_drop(&mut s, &a, Drop::Into(f.clone()));
        let Some(Item::Folder(folder)) = s.workspace.find_item(&f) else {
            panic!()
        };
        assert_eq!(folder.children[0].id(), &a);
        apply_drop(&mut s, &b, Drop::After(f.clone()));
        assert_eq!(s.workspace.items.last().unwrap().id(), &b);
    }
}
