use glam::Vec2;
use palantir::{
    Background, Brush, Button, ButtonTheme, Color, Configure, ContextMenu, Corners, HostHandle,
    MenuItem, Panel, Shadow, Sizing, Spacing, Stroke, Ui, WidgetLook,
};

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

/// Flat trigger styling for menu-bar entries: transparent fill at
/// rest, hover-only background, no border or shadow, tighter padding
/// than the default Button. Matches the conventional menu-bar look
/// (Figma / VS Code / macOS) — the trigger reads as text until the
/// pointer is over it. Built once per call; cheap.
fn menu_trigger_theme() -> ButtonTheme {
    const HOVER_BG: Color = Color::hex(0x2a2a30);
    const PRESSED_BG: Color = Color::hex(0x3a3a44);
    let flat = |fill: Brush| WidgetLook {
        background: Some(Background {
            fill,
            stroke: Stroke::ZERO,
            corners: Corners::all(4.0),
            shadow: Shadow::NONE,
        }),
        text: None,
    };
    ButtonTheme {
        normal: flat(Brush::TRANSPARENT),
        hovered: flat(HOVER_BG.into()),
        pressed: flat(PRESSED_BG.into()),
        disabled: flat(Brush::TRANSPARENT),
        padding: Spacing::xy(8.0, 4.0),
        margin: Spacing::ZERO,
        anim: None,
    }
}

fn file_menu(ui: &mut Ui, host: Option<&HostHandle>) -> Option<FileAction> {
    let trigger = Button::new()
        .label("File")
        .style(menu_trigger_theme())
        .show(ui)
        .snapshot();
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
