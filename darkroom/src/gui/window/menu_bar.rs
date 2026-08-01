use glam::Vec2;
use palantir::{Button, Configure, ContextMenu, MenuItem, Panel, PopupHandle, Sizing, Spacing, Ui};

use crate::core::document::TabRef;
use crate::core::document::dock::DockOp;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::requests::Requests;

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
///
/// A pick goes onto `out` in whichever tier it belongs to: the file lifecycle
/// and `Quit` as [`AppCommand`]s for `App` to run after the pass, Preferences
/// as the [`DockOp`] that opens its tab.
pub(crate) fn show(ui: &mut Ui, out: &mut Requests) {
    Panel::hstack()
        .auto_id()
        .size((Sizing::HUG, Sizing::HUG))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .show(ui, |ui| {
            file_menu(ui, out);
        });
}

/// One menu-bar dropdown: a flat trigger button that toggles a
/// `ContextMenu` of [`MenuItem`] rows. `build` populates the popup and raises
/// whatever a pick means. Centralizes the trigger + anchor + open plumbing so
/// each menu is just its label + rows.
fn dropdown(ui: &mut Ui, label: &'static str, build: impl FnOnce(&mut Ui, &PopupHandle)) {
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
    ContextMenu::for_id(trigger.id).show(ui, |ui, popup| {
        build(ui, popup);
    });
}

fn file_menu(ui: &mut Ui, out: &mut Requests) {
    dropdown(ui, "File", |ui, popup| {
        if MenuItem::new("New").show(ui, popup).left.clicked() {
            out.push_app(AppCommand::File(FileCommand::New));
        }
        if MenuItem::new("Load…").show(ui, popup).left.clicked() {
            out.push_app(AppCommand::File(FileCommand::Load));
        }
        if MenuItem::new("Save").show(ui, popup).left.clicked() {
            out.push_app(AppCommand::File(FileCommand::Save));
        }
        if MenuItem::new("Save As…").show(ui, popup).left.clicked() {
            out.push_app(AppCommand::File(FileCommand::SaveAs));
        }
        MenuItem::separator().show(ui);
        if MenuItem::new("Preferences").show(ui, popup).left.clicked() {
            out.push_view(DockOp::OpenTab {
                tab: TabRef::Preferences,
            });
        }
        MenuItem::separator().show(ui);
        if MenuItem::new("Quit").show(ui, popup).left.clicked() {
            out.push_app(AppCommand::Quit);
        }
    });
}
