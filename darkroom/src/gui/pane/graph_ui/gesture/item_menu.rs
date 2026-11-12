//! [`ItemMenu`]: the popup a right-click on a canvas item opens.

use std::collections::BTreeSet;

use palantir::Ui;

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;

use crate::gui::pane::graph_ui::gesture::anchored_menu::{AnchoredMenu, PopupShape};

/// A context popup latched by a right-click on a canvas item, a node body or a tie. Wraps
/// [`AnchoredMenu`] with the one per-open extra it needs: the item `K` the menu was opened on,
/// which the open latched frames before the pick that needs it.
///
/// The caller still owns the items, where a pick goes (an `AppCommand` or a `GraphIntent`), and
/// which item was right-clicked, which the input stage settles.
#[derive(Debug)]
pub(crate) struct ItemMenu<K> {
    menu: AnchoredMenu,
    /// The item whose widget opened the menu. Set with the anchor and read back by
    /// [`Self::show`]; left set after a close, as the wrapped `AnchoredMenu` gates every read.
    target: Option<K>,
}

/// Hand-written: `derive` would demand `K: Default`, and a closed menu holds
/// no target to default.
impl<K> Default for ItemMenu<K> {
    fn default() -> Self {
        Self {
            menu: AnchoredMenu::default(),
            target: None,
        }
    }
}

impl<K: Copy> ItemMenu<K> {
    /// Close the menu. `target` is left as it was, as every read of it is gated by the wrapped
    /// [`AnchoredMenu`] being open.
    pub(crate) const fn reset(&mut self) {
        self.menu.reset();
    }

    pub(crate) const fn is_open(&self) -> bool {
        self.menu.is_open()
    }

    /// Close the menu when the document no longer `holds` its target, as a Delete or an undo
    /// leaves it while the popup stays open: its picks would apply to nothing, or to whatever the
    /// selection is. Called before each [`Self::show`].
    pub(crate) fn retain(&mut self, holds: impl FnOnce(K) -> bool) {
        if self.menu.is_open()
            && let Some(target) = self.target
            && !holds(target)
        {
            self.menu.reset();
        }
    }

    /// Open on `target`, anchored at the pointer, and select it if it is not part of the
    /// selection, so the chosen action targets a coherent set ("select then act"). Returns that
    /// selection edit; a pick lands frames later, so the selection is committed well before it
    /// is read back.
    ///
    /// `target` comes from a click on its widget, so the document holds it at the open;
    /// [`Self::retain`] keeps that true across the frames the popup stays. The popup records on the
    /// next [`Self::show`], in the input stage's own frame.
    pub(crate) fn open_on(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        target: K,
    ) -> Option<GraphIntent>
    where
        K: Into<ItemId>,
    {
        // A press that opened the menu has a pointer position by construction; `None` only when
        // the pointer left the window between the click and this read.
        let at = ui.pointer_pos()?;
        self.target = Some(target);
        self.menu.open_at(at);
        let item = target.into();
        (!graph_ctx.is_selected(item)).then(|| GraphIntent::SetSelection {
            to: BTreeSet::from([item]),
        })
    }

    /// Show the menu — see [`AnchoredMenu::show`] for the close rules. `body`
    /// records the items against the item the open latched and returns the
    /// pick, which comes back paired with that item.
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        body: impl FnOnce(&mut Ui, K) -> Option<T>,
    ) -> Option<Pick<K, T>> {
        let target = self.target?;
        let shape = PopupShape {
            id_salt,
            max_height: None,
        };
        let choice = self.menu.show(ui, shape, |ui| body(ui, target))?;
        Some(Pick { target, choice })
    }
}

/// A pick from a [`ItemMenu`], carrying the item the menu was opened on.
/// The two travel together because a pick means nothing without the item it applies to, which
/// was latched frames earlier, not read off the pointer or the selection at click time.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pick<K, T> {
    pub(crate) target: K,
    pub(crate) choice: T,
}
