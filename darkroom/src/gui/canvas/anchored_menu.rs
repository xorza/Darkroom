use glam::Vec2;
use palantir::{ClickOutside, Configure, Popup, PopupHandle, Sizing, Ui};

use crate::core::document::GraphRef;

/// Shared open/close lifecycle + chrome for the canvas's anchored context
/// popups (the node menu, graph-badge menu, and new-node palette). Owns
/// only the surface-space anchor, the graph pane it belongs to, and the
/// dismiss bookkeeping; each caller stores its own per-open extras (target
/// node, drop position, …) as plain fields set at open-time and read at
/// pick-time.
///
/// Centralizes what those three controllers used to each re-implement: the
/// Esc-to-close guard, the identical `Popup` chrome (the `context_menu`
/// theme slot's panel, padding, and width floor, hug sizing,
/// click-outside dismiss), and the "a pick or an outside dismiss closes
/// the menu" resolution.
///
/// **One pane records it.** Every visible graph pane runs its own scan +
/// `show` pass, so without the `graph` latch an open popup would be
/// recorded once per pane — the same widget ids twice in one frame.
/// [`Self::show`] answers `None` for every pane but the one that opened it.
#[derive(Default, Debug)]
pub(super) struct AnchoredMenu {
    anchor: Option<Vec2>,
    /// The pane that opened the menu; `None` while it's closed.
    graph: Option<GraphRef>,
}

impl AnchoredMenu {
    /// Open (or re-anchor) the menu at a surface-space point, on `graph`'s
    /// pane.
    pub(super) fn open_at(&mut self, anchor: Vec2, graph: GraphRef) {
        self.anchor = Some(anchor);
        self.graph = Some(graph);
    }

    /// Show the menu when open **and** `graph` is the pane that opened it,
    /// recording `body` inside the shared popup chrome. `body` records the
    /// items and returns the pick (if any); returning `Some` — or an Esc /
    /// outside-click dismiss — closes the menu. The pick is handed back for
    /// the caller to act on. `max_height` caps the popup so a tall body
    /// wraps/scrolls (the new-node palette); `None` hugs the content (the
    /// small context menus).
    pub(super) fn show<T>(
        &mut self,
        ui: &mut Ui,
        graph: GraphRef,
        id_salt: &'static str,
        max_height: Option<f32>,
        body: impl FnOnce(&mut Ui, &PopupHandle) -> Option<T>,
    ) -> Option<T> {
        if self.graph != Some(graph) {
            return None;
        }
        let anchor = self.anchor?;
        // Esc dismissal is owned by the `Dismiss` popup below (folds into
        // `resp.dismissed`) — no separate `escape_pressed` here.
        //
        // Chrome, padding, and the width floor all come off the same theme
        // slot `ContextMenu::show` reads, so a canvas menu and a menu-bar
        // menu are the same object; these popups only opt out of
        // `ContextMenu` for its per-trigger open lifecycle, not its look.
        let ctx = &ui.theme.context_menu;
        let chrome = ctx.panel.clone();
        let padding = ctx.padding;
        let min_width = ctx.min_width;
        let mut pick = None;
        let mut popup = Popup::anchored_to(anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt(id_salt)
            .size((Sizing::HUG, Sizing::HUG))
            .min_size((min_width, 0.0))
            .padding(padding);
        if let Some(h) = max_height {
            popup = popup.max_size((f32::INFINITY, h));
        }
        let resp = popup.show(ui, |ui, popup| {
            pick = body(ui, popup);
        });
        if pick.is_some() || resp.dismissed || resp.close_requested {
            self.anchor = None;
            self.graph = None;
        }
        pick
    }
}
