use glam::Vec2;
use palantir::{Button, Configure, ContextMenu, HostHandle, MenuItem, Panel, Sizing, Spacing, Ui};

/// Document-level action surfaced by the File menu. `App` handles the
/// side effect (file dialog + read/write + doc swap) outside the
/// record pass — keeps `menu_bar` decoupled from `Document` /
/// `ActionStack` and lets the dialog block the UI thread without
/// holding any borrows from the active frame.
#[derive(Clone, Copy, Debug)]
pub enum FileAction {
    New,
    Load,
    Save,
}

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
/// `Quit` calls into [`HostHandle::quit`]; document items return a
/// [`FileAction`] for `App` to consume.
pub fn show(ui: &mut Ui, host: Option<&HostHandle>) -> Option<FileAction> {
    let mut action = None;
    Panel::hstack()
        .auto_id()
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .show(ui, |ui| {
            action = file_menu(ui, host);
        });
    action
}

fn file_menu(ui: &mut Ui, host: Option<&HostHandle>) -> Option<FileAction> {
    let trigger = Button::new().label("File").show(ui).snapshot();
    if trigger.clicked()
        && let Some(rect) = trigger.rect()
    {
        ContextMenu::open(ui, trigger.widget_id(), Vec2::new(rect.min.x, rect.max().y));
    }
    let mut action = None;
    ContextMenu::for_id(trigger.widget_id()).show(ui, |ui, popup| {
        if MenuItem::new("New").show(ui, popup).clicked() {
            action = Some(FileAction::New);
        }
        if MenuItem::new("Load…").show(ui, popup).clicked() {
            action = Some(FileAction::Load);
        }
        if MenuItem::new("Save…").show(ui, popup).clicked() {
            action = Some(FileAction::Save);
        }
        MenuItem::separator(ui);
        if MenuItem::new("Quit").show(ui, popup).clicked()
            && let Some(h) = host
        {
            h.quit();
        }
    });
    action
}
