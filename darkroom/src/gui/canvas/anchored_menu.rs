use aperture::{ClickOutside, Configure, Popup, PopupHandle, Sizing, Spacing, Ui};
use glam::Vec2;

/// Shared open/close lifecycle + chrome for the canvas's anchored context
/// popups (the node menu, subgraph-badge menu, and new-node palette). Owns
/// only the surface-space anchor and the dismiss bookkeeping; each caller
/// stores its own per-open extras (target node, drop position, …) as plain
/// fields set at open-time and read at pick-time.
///
/// Centralizes what those three controllers used to each re-implement: the
/// Esc-to-close guard, the identical `Popup` chrome (context-menu
/// background, hug sizing, 6 px padding, click-outside dismiss), and the
/// "a pick or an outside dismiss closes the menu" resolution.
#[derive(Default, Debug)]
pub(crate) struct AnchoredMenu {
    anchor: Option<Vec2>,
}

impl AnchoredMenu {
    /// Open (or re-anchor) the menu at a surface-space point.
    pub(crate) fn open_at(&mut self, anchor: Vec2) {
        self.anchor = Some(anchor);
    }

    /// Show the menu when open, recording `body` inside the shared popup
    /// chrome. `body` records the items and returns the pick (if any);
    /// returning `Some` — or an Esc / outside-click dismiss — closes the
    /// menu. The pick is handed back for the caller to act on. `max_height`
    /// caps the popup so a tall body wraps/scrolls (the new-node palette);
    /// `None` hugs the content (the small context menus).
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        max_height: Option<f32>,
        body: impl FnOnce(&mut Ui, &PopupHandle) -> Option<T>,
    ) -> Option<T> {
        let anchor = self.anchor?;
        // Esc dismissal is owned by the `Dismiss` popup below (folds into
        // `resp.dismissed`) — no separate `escape_pressed` here.
        let chrome = ui.theme.context_menu.panel.clone();
        let mut pick = None;
        let mut popup = Popup::anchored_to(anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt(id_salt)
            .size((Sizing::Hug, Sizing::Hug))
            .padding(Spacing::all(6.0));
        if let Some(h) = max_height {
            popup = popup.max_size((f32::INFINITY, h));
        }
        let resp = popup.show(ui, |ui, popup| {
            pick = body(ui, popup);
        });
        if pick.is_some() || resp.dismissed || resp.close_requested {
            self.anchor = None;
        }
        pick
    }
}

/// The open-time target a context menu latches, read while resolving its
/// pick — the shape every [`AnchoredMenu`] caller wraps around it (the node
/// menu's clicked node, the subgraph menu's badge node). `set` overwrites
/// any previous target (a fresh secondary-click always wins); `get` reads
/// without consuming, since a pick fires the same frame the menu is still
/// showing the latched target.
#[derive(Default, Debug)]
pub(crate) struct Latched<T>(Option<T>);

impl<T: Copy> Latched<T> {
    pub(crate) fn set(&mut self, target: T) {
        self.0 = Some(target);
    }

    pub(crate) fn get(&self) -> Option<T> {
        self.0
    }
}
