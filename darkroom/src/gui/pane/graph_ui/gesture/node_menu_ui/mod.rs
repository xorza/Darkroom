use palantir::Ui;
use scenarium::NodeId;

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::command::menu::{self, NODE_MENU, Unavailable};
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::gesture::item_menu::ItemMenu;

/// Right-click on a node body → a popup of the selection's commands, [`NODE_MENU`]: a
/// right-click selects the node it lands on first, so each row acts on what the menu opened on,
/// and a row whose command the selection does not take is left out. On an instance that is its
/// compound's commands too, where the user meets it: Open, Expand, Make Unique, Make Local while
/// it follows the library, and Locked. The trigger scan, the per-open node latch, and the popup
/// lifecycle are [`ItemMenu`]'s.
#[derive(Default, Debug)]
pub(crate) struct NodeMenuUi {
    menu: ItemMenu<NodeId>,
}

impl NodeMenuUi {
    /// Close the menu.
    pub(crate) const fn reset(&mut self) {
        self.menu.reset();
    }

    /// Whether the menu is open.
    pub(crate) const fn in_flight(&self) -> bool {
        self.menu.is_open()
    }

    /// Open the menu on `node`, which the input stage just saw right-clicked; see
    /// [`ItemMenu::open_on`]. Returns the selection edit, when it needs one.
    pub(crate) fn open_on(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        node: NodeId,
    ) -> Option<GraphIntent> {
        self.menu.open_on(ui, graph_ctx, node)
    }

    /// Record the menu, and raise the command of a picked row onto the stage's requests.
    ///
    /// Opening is [`Self::open_on`]'s; this only ever shows a menu already latched.
    pub(crate) fn record(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
        self.menu.retain(|node_id| graph_ctx.contains(node_id));
        self.menu.show(ui, "node_body_menu", |ui, _| {
            menu::record(ui, NODE_MENU, graph_ctx, out, Unavailable::Hidden);
            None::<()>
        });
    }
}

#[cfg(test)]
mod tests;
