use glam::Vec2;
use palantir::{ClickOutside, Configure, Popup, PopupHandle, Sizing, Ui};
use scenarium::NodeId;

use crate::gui::canvas::gesture_slot::GestureSlot;
use crate::gui::canvas::hits::CanvasHits;
use crate::gui::graph_scope::GraphScope;

/// Shared open/close lifecycle + chrome for the canvas's anchored context
/// popups (the node menu, graph-badge menu, and new-node palette). Owns
/// only the surface-space anchor and the
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
#[derive(Default, Debug)]
pub(super) struct AnchoredMenu {
    /// The surface-space anchor the menu opened at.
    anchor: GestureSlot<Vec2>,
}

impl AnchoredMenu {
    /// Open (or re-anchor) the menu at a surface-space point.
    pub(super) fn open_at(&mut self, anchor: Vec2) {
        self.anchor.latch(anchor);
    }

    /// Whether the menu is open.
    ///
    /// For a caller with per-frame setup to skip: [`Self::show`] answers
    /// `None` for a closed menu anyway, but only *after* its arguments
    /// have been built.
    pub(super) fn open_on(&self) -> bool {
        self.anchor.get().is_some()
    }

    /// Show the menu when open, recording `body` inside the shared popup chrome. `body` records the
    /// items and returns the pick (if any); returning `Some` — or an Esc /
    /// outside-click dismiss — closes the menu. The pick is handed back for
    /// the caller to act on. `max_height` caps the popup so a tall body
    /// wraps/scrolls (the new-node palette); `None` hugs the content (the
    /// small context menus).
    pub(super) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        max_height: Option<f32>,
        body: impl FnOnce(&mut Ui, &PopupHandle) -> Option<T>,
    ) -> Option<T> {
        // `None` for a menu that isn't open.
        let anchor = *self.anchor.get()?;
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
        let gap = ctx.gap;
        let mut pick = None;
        let mut popup = Popup::anchored_to(anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt(id_salt)
            .size((Sizing::HUG, Sizing::HUG))
            .min_size((min_width, 0.0))
            .padding(padding)
            .gap(gap);
        if let Some(h) = max_height {
            popup = popup.max_size((f32::INFINITY, h));
        }
        let resp = popup.show(ui, |ui, popup| {
            pick = body(ui, popup);
        });
        if pick.is_some() || resp.dismissed || resp.close_requested {
            self.anchor.clear();
        }
        pick
    }
}

/// A context popup latched by a right-click on a node's body — the whole shape
/// a canvas node menu takes, from the trigger scan to the pick. Wraps
/// [`AnchoredMenu`] with the one per-open extra it needs: the node the menu was
/// opened on, which the open latched frames before the pick that needs it.
///
/// What the caller still owns is the items and where a pick goes — an
/// `AppCommand`, a `GraphIntent`, or a stash for the `Editor` to resolve. Which
/// nodes offer the menu at all is settled by [`CanvasHits::scan`].
#[derive(Default, Debug)]
pub(super) struct NodeContextMenu {
    menu: AnchoredMenu,
    /// The node whose widget opened the menu. Set with the anchor and read
    /// back by [`Self::show`]; left set after a close, which is unreachable
    /// because the wrapped `AnchoredMenu` is what gates every read of it.
    node_id: Option<NodeId>,
}

impl NodeContextMenu {
    /// Open on this frame's secondary click, anchored at the pointer, and
    /// report the node it opened on (`None` on every other frame).
    ///
    /// The hit comes from this frame's sweep, which already applied the trigger
    /// widget's draw guard; all that is left here is confirming the node still
    /// belongs to `graph_scope` — the sweep ran against last frame's projection.
    pub(super) fn latch(
        &mut self,
        ui: &mut Ui,
        hits: &CanvasHits,
        graph_scope: GraphScope<'_>,
    ) -> Option<NodeId> {
        let clicked = hits.menu().filter(|&id| graph_scope.contains(id))?;
        // A press that opened the menu has a pointer position by construction;
        // the `?` is only for the frames where the pointer left the window
        // between the click and this read.
        let at = ui.pointer_pos()?;
        self.node_id = Some(clicked);
        self.menu.open_at(at);
        Some(clicked)
    }

    /// Show the menu — see [`AnchoredMenu::show`] for the close rules. `body`
    /// records the items against the node the open latched and returns the
    /// pick, which comes back paired with that node.
    pub(super) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        body: impl FnOnce(&mut Ui, &PopupHandle, NodeId) -> Option<T>,
    ) -> Option<NodePick<T>> {
        let node_id = self.node_id?;
        let choice = self
            .menu
            .show(ui, id_salt, None, |ui, popup| body(ui, popup, node_id))?;
        Some(NodePick { node_id, choice })
    }
}

/// A pick from a [`NodeContextMenu`], carrying the node the menu was opened
/// on. The two travel together because a pick means nothing without the node
/// it applies to, and that node was latched frames earlier — not read back off
/// whatever the pointer or the selection happens to be at click time.
#[derive(Clone, Copy, Debug)]
pub(super) struct NodePick<T> {
    pub(super) node_id: NodeId,
    pub(super) choice: T,
}
