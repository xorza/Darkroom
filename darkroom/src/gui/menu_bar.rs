use glam::Vec2;
use palantir::{Button, Configure, ContextMenu, MenuItem, Panel, PopupHandle, Sizing, Spacing, Ui};

use crate::gui::HostHandle;
use crate::gui::app::AppCommand;

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
/// `Quit` calls into [`HostHandle::quit`]; everything else returns a
/// [`AppCommand`] for `App` to consume.
pub(crate) fn show(ui: &mut Ui, host: Option<&HostHandle>) -> Option<AppCommand> {
    let mut command = None;
    Panel::hstack()
        .auto_id()
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .show(ui, |ui| {
            command = file_menu(ui, host);
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
    let trigger = Button::new()
        .label(label)
        .style(ui.theme.menu_button.clone())
        .show(ui)
        .snapshot();
    if trigger.clicked()
        && let Some(rect) = trigger.rect()
    {
        ContextMenu::open(ui, trigger.widget_id(), Vec2::new(rect.min.x, rect.max().y));
    }
    let mut command = None;
    ContextMenu::for_id(trigger.widget_id()).show(ui, |ui, popup| {
        command = build(ui, popup);
    });
    command
}

fn file_menu(ui: &mut Ui, host: Option<&HostHandle>) -> Option<AppCommand> {
    dropdown(ui, "File", |ui, popup| {
        let mut command = None;
        if MenuItem::new("New").show(ui, popup).clicked() {
            command = Some(AppCommand::NewDocument);
        }
        if MenuItem::new("Load…").show(ui, popup).clicked() {
            command = Some(AppCommand::LoadDocument);
        }
        if MenuItem::new("Save").show(ui, popup).clicked() {
            command = Some(AppCommand::SaveDocument);
        }
        if MenuItem::new("Save As…").show(ui, popup).clicked() {
            command = Some(AppCommand::SaveDocumentAs);
        }
        MenuItem::separator(ui);
        if MenuItem::new("Preferences").show(ui, popup).clicked() {
            command = Some(AppCommand::OpenPreferences);
        }
        MenuItem::separator(ui);
        if MenuItem::new("Quit").show(ui, popup).clicked()
            && let Some(h) = host
        {
            h.quit();
        }
        command
    })
}

/// Subgraph import/export/promote actions. Hidden from the menu bar for
/// now — kept intact so it can be re-enabled without rebuilding it.
#[allow(dead_code)]
fn subgraph_menu(ui: &mut Ui) -> Option<AppCommand> {
    dropdown(ui, "Subgraph", |ui, popup| {
        let mut command = None;
        if MenuItem::new("Export…").show(ui, popup).clicked() {
            command = Some(AppCommand::ExportSubgraph);
        }
        if MenuItem::new("Import…").show(ui, popup).clicked() {
            command = Some(AppCommand::ImportSubgraph);
        }
        if MenuItem::new("Promote to Library…")
            .show(ui, popup)
            .clicked()
        {
            command = Some(AppCommand::PromoteSubgraph);
        }
        command
    })
}
