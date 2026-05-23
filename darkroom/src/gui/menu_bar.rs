use glam::Vec2;
use palantir::{
    Background, Button, Color, Configure, ContextMenu, HostHandle, MenuItem, Panel, Sizing,
    Spacing, Ui,
};

const MENUBAR_BG: Color = Color::hex(0x141414);

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
/// `Quit` calls into [`HostHandle::quit`]; the other items are stubs.
pub fn show(ui: &mut Ui, host: Option<&HostHandle>) {
    Panel::hstack()
        .auto_id()
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .background(Background {
            fill: MENUBAR_BG.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            file_menu(ui, host);
        });
}

fn file_menu(ui: &mut Ui, host: Option<&HostHandle>) {
    let trigger = Button::new().label("File").show(ui).snapshot();
    if trigger.clicked()
        && let Some(rect) = trigger.rect()
    {
        ContextMenu::open(ui, trigger.widget_id(), Vec2::new(rect.min.x, rect.max().y));
    }
    ContextMenu::for_id(trigger.widget_id()).show(ui, |ui, popup| {
        if MenuItem::new("New").show(ui, popup).clicked() {
            // TODO: clear document
        }
        if MenuItem::new("Load…").show(ui, popup).clicked() {
            // TODO: file picker + load
        }
        if MenuItem::new("Save…").show(ui, popup).clicked() {
            // TODO: file picker + save
        }
        MenuItem::separator(ui);
        if MenuItem::new("Quit").show(ui, popup).clicked()
            && let Some(h) = host
        {
            h.quit();
        }
    });
}
