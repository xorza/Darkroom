use glam::Vec2;
use palantir::{Button, Configure, ContextMenu, MenuItem, Panel, PopupHandle, Sizing, Spacing, Ui};

use crate::core::document::TabRef;
use crate::core::document::dock::DockOp;
use crate::core::edit::intent::sink::Intents;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
///
/// A pick goes wherever its tier does: the file lifecycle and `Quit`
/// return an [`AppCommand`] for `App` to run after the pass, while
/// Preferences is a pane arrangement like any other and queues its
/// [`DockOp`] onto `out`.
pub(crate) fn show(ui: &mut Ui, out: &mut Intents) -> Option<AppCommand> {
    let mut command = None;
    Panel::hstack()
        .auto_id()
        .size((Sizing::HUG, Sizing::HUG))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .show(ui, |ui| {
            if let Some(file_command) = file_menu(ui, out) {
                command = Some(file_command);
            }
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

fn file_menu(ui: &mut Ui, out: &mut Intents) -> Option<AppCommand> {
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
        MenuItem::separator().show(ui);
        if MenuItem::new("Preferences").show(ui, popup).left.clicked() {
            out.push_dock(DockOp::OpenTab {
                tab: TabRef::Preferences,
            });
        }
        MenuItem::separator().show(ui);
        if MenuItem::new("Quit").show(ui, popup).left.clicked() {
            command = Some(AppCommand::Quit);
        }
        command
    })
}
