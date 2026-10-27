//! [`ContextMenu`]: the popup a right-click on a canvas item opens.

use palantir::CloseHandle;
use palantir::prelude::*;

use crate::gui::pane::graph_ui::draw::anchored_menu::{AnchoredMenu, PopupShape};

/// A context popup latched by a right-click on a canvas item, a node body or
/// a tie — the whole shape a canvas item menu takes, from the trigger to the
/// pick. Wraps [`AnchoredMenu`] with the one per-open extra it needs: the
/// item `K` the menu was opened on, which the open latched frames before the
/// pick that needs it.
///
/// What the caller still owns is the items and where a pick goes — an
/// `AppCommand` or a `GraphIntent`. Which item was right-clicked is settled by
/// its draw, which reports it, as
/// [`NodeDrawOutcome::menu_opened`](crate::gui::pane::graph_ui::node_ui::NodeDrawOutcome)
/// does for a node.
#[derive(Debug)]
pub(crate) struct ContextMenu<K> {
    menu: AnchoredMenu,
    /// The item whose widget opened the menu. Set with the anchor and read
    /// back by [`Self::show`]; left set after a close, which is unreachable
    /// because the wrapped `AnchoredMenu` is what gates every read of it.
    target: Option<K>,
}

/// Hand-written: `derive` would demand `K: Default`, and a closed menu holds
/// no target to default.
impl<K> Default for ContextMenu<K> {
    fn default() -> Self {
        Self {
            menu: AnchoredMenu::default(),
            target: None,
        }
    }
}

impl<K: Copy> ContextMenu<K> {
    /// Close the menu. `target` is left as it was — every read of it is
    /// gated by the wrapped [`AnchoredMenu`] being open.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
    }

    pub(crate) const fn is_open(&self) -> bool {
        self.menu.is_open()
    }

    /// Open on `target`, anchored at the pointer, and report whether it did.
    ///
    /// `target` comes from the record pass that drew it, so the document
    /// holds it by construction — nothing left to confirm. Called *after*
    /// that draw, because the menu is the canvas's state and the draw holds it
    /// shared; the popup itself records on the next pass, which a right-click
    /// guarantees (action input always earns one).
    pub(crate) fn open_on(&mut self, ui: &mut Ui, target: K) -> bool {
        // A press that opened the menu has a pointer position by construction;
        // this is only for the frames where the pointer left the window
        // between the click and this read.
        let Some(at) = ui.pointer_pos() else {
            return false;
        };
        self.target = Some(target);
        self.menu.open_at(at);
        true
    }

    /// Show the menu — see [`AnchoredMenu::show`] for the close rules. `body`
    /// records the items against the item the open latched and returns the
    /// pick, which comes back paired with that item.
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        body: impl FnOnce(&mut Ui, &CloseHandle, K) -> Option<T>,
    ) -> Option<Pick<K, T>> {
        let target = self.target?;
        let shape = PopupShape {
            id_salt,
            max_height: None,
        };
        let choice = self
            .menu
            .show(ui, shape, |ui, popup| body(ui, popup, target))?;
        Some(Pick { target, choice })
    }
}

/// A pick from a [`ContextMenu`], carrying the item the menu was opened on.
/// The two travel together because a pick means nothing without the item it
/// applies to, and that item was latched frames earlier — not read back off
/// whatever the pointer or the selection happens to be at click time.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pick<K, T> {
    pub(crate) target: K,
    pub(crate) choice: T,
}
