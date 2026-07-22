use aperture::{Button, Configure, ContextMenu, MenuItem, Panel, PopupHandle, Sizing, Spacing, Ui};
use glam::Vec2;

use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::graph::GraphCommand;
use crate::gui::app::commands::shell::ShellCommand;

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
/// Every item — `Quit` included — returns an [`AppCommand`] for `App` to
/// consume; `App` routes `Quit` through its unsaved-changes check.
pub(crate) fn show(ui: &mut Ui) -> Option<AppCommand> {
    let mut command = None;
    Panel::hstack()
        .auto_id()
        .size((Sizing::HUG, Sizing::HUG))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .show(ui, |ui| {
            command = file_menu(ui);
        });
    command
}

/// One menu-bar dropdown: a flat trigger button that toggles a
/// `ContextMenu` of [`MenuItem`] rows. `build` populates the popup and
/// returns the chosen command, if any. Centralizes the trigger +
/// anchor + open plumbing so each menu is just its label + rows.
fn dropdown(
    ui: &mut Ui,
    label: &'static str,
    build: impl FnOnce(&mut Ui, &PopupHandle) -> Option<AppCommand>,
) -> Option<AppCommand> {
    let menu_button = ui.theme.menu_button.clone();
    let trigger = Button::new()
        .label(label)
        .style(&menu_button)
        .show(ui)
        .snapshot();
    if trigger.left.clicked()
        && let Some(rect) = trigger.rect
    {
        ContextMenu::open(ui, trigger.id, Vec2::new(rect.min.x, rect.max().y));
    }
    let mut command = None;
    ContextMenu::for_id(trigger.id).show(ui, |ui, popup| {
        command = build(ui, popup);
    });
    command
}

fn file_menu(ui: &mut Ui) -> Option<AppCommand> {
    dropdown(ui, "File", |ui, popup| {
        let mut command = None;
        if MenuItem::new("New").show(ui, popup).left.clicked() {
            command = Some(AppCommand::File(FileCommand::New));
        }
        if MenuItem::new("Load…").show(ui, popup).left.clicked() {
            command = Some(AppCommand::File(FileCommand::Load));
        }
        if MenuItem::new("Save").show(ui, popup).left.clicked() {
            command = Some(AppCommand::File(FileCommand::Save));
        }
        if MenuItem::new("Save As…").show(ui, popup).left.clicked() {
            command = Some(AppCommand::File(FileCommand::SaveAs));
        }
        MenuItem::separator(ui);
        if MenuItem::new("Preferences").show(ui, popup).left.clicked() {
            command = Some(AppCommand::Shell(ShellCommand::OpenPreferences));
        }
        MenuItem::separator(ui);
        if MenuItem::new("Quit").show(ui, popup).left.clicked() {
            command = Some(AppCommand::Shell(ShellCommand::Quit));
        }
        command
    })
}

/// Graph-template import/export actions. Hidden from the menu bar for
/// now — kept intact so it can be re-enabled without rebuilding it.
#[allow(dead_code)]
fn graph_menu(ui: &mut Ui) -> Option<AppCommand> {
    dropdown(ui, "Graph", |ui, popup| {
        let mut command = None;
        if MenuItem::new("Export Graph Template…")
            .show(ui, popup)
            .left
            .clicked()
        {
            command = Some(AppCommand::Graph(GraphCommand::ExportGraphTemplate));
        }
        if MenuItem::new("Import Graph Template into Document…")
            .show(ui, popup)
            .left
            .clicked()
        {
            command = Some(AppCommand::Graph(
                GraphCommand::ImportGraphTemplateIntoDocument,
            ));
        }
        if MenuItem::new("Import Graph Template into Library…")
            .show(ui, popup)
            .left
            .clicked()
        {
            command = Some(AppCommand::Graph(
                GraphCommand::ImportGraphTemplateIntoLibrary,
            ));
        }
        command
    })
}
