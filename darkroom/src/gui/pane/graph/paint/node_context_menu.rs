//! [`NodeContextMenu`]: the popup a right-click on a node body opens.

use palantir::CloseHandle;
use palantir::prelude::*;
use scenarium::NodeId;

use crate::gui::pane::graph::paint::anchored_menu::AnchoredMenu;

/// A context popup latched by a right-click on a node's body — the whole shape
/// a canvas node menu takes, from the trigger scan to the pick. Wraps
/// [`AnchoredMenu`] with the one per-open extra it needs: the node the menu was
/// opened on, which the open latched frames before the pick that needs it.
///
/// What the caller still owns is the items and where a pick goes — an
/// `AppCommand` or a `GraphIntent`. Every
/// node offers the menu; which one was right-clicked is settled by the node
/// draw, which reports it as
/// [`NodeDrawOutcome::menu_opened`](crate::gui::pane::graph::node::NodeDrawOutcome).
#[derive(Default, Debug)]
pub(crate) struct NodeContextMenu {
    menu: AnchoredMenu,
    /// The node whose widget opened the menu. Set with the anchor and read
    /// back by [`Self::show`]; left set after a close, which is unreachable
    /// because the wrapped `AnchoredMenu` is what gates every read of it.
    node_id: Option<NodeId>,
}

impl NodeContextMenu {
    /// Close the menu. `node_id` is left as it was — every read of it is
    /// gated by the wrapped [`AnchoredMenu`] being open.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.menu.is_open()
    }

    /// Open on `node`, anchored at the pointer, and report whether it did.
    ///
    /// `node` comes from the record pass that drew it, so it is in the graph
    /// by construction — nothing left to confirm. Called *after* that draw,
    /// because the menu is the canvas's state and the draw holds it shared;
    /// the popup itself records on the next pass, which a right-click
    /// guarantees (action input always earns one).
    pub(crate) fn open_on(&mut self, ui: &mut Ui, node: NodeId) -> bool {
        // A press that opened the menu has a pointer position by construction;
        // this is only for the frames where the pointer left the window
        // between the click and this read.
        let Some(at) = ui.pointer_pos() else {
            return false;
        };
        self.node_id = Some(node);
        self.menu.open_at(at);
        true
    }

    /// Show the menu — see [`AnchoredMenu::show`] for the close rules. `body`
    /// records the items against the node the open latched and returns the
    /// pick, which comes back paired with that node.
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        body: impl FnOnce(&mut Ui, &CloseHandle, NodeId) -> Option<T>,
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
pub(crate) struct NodePick<T> {
    pub(crate) node_id: NodeId,
    pub(crate) choice: T,
}
