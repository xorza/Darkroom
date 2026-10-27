use palantir::{MenuItem, MenuSeparator, Ui};

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use scenarium::NodeId;

use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::gesture::item_menu::ItemMenu;

/// Right-click on a node body → a small popup with actions on the node.
/// The trigger scan, the per-open node latch, and the popup lifecycle are all
/// [`ItemMenu`]'s. "Run to this node" needs only the clicked node's id
/// and surfaces an [`AppCommand`]; the structural picks read the live
/// selection off the context and push their intents onto the frame's queue,
/// the same two builders the Ctrl+D and Delete chords drive.
#[derive(Default, Debug)]
pub(crate) struct NodeMenuUi {
    menu: ItemMenu<NodeId>,
}

/// A menu pick before routing. `Run` names the node the menu opened on; the
/// structural picks carry only their kind, because their target is the
/// selection (a right-click selects the node it landed on first).
#[derive(Copy, Clone, Debug)]
enum MenuChoice {
    Run,
    Duplicate { incoming: IncomingWires },
    Remove,
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

    /// Open the menu on `node`, which the record pass just saw right-clicked; see
    /// [`ItemMenu::open_on`]. Returns the selection edit, when it needs one.
    pub(crate) fn open_on(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        node: NodeId,
    ) -> Option<GraphIntent> {
        self.menu.open_on(ui, graph_ctx, node)
    }

    /// Record the menu and resolve this frame's pick onto `out` — a run as
    /// the [`AppCommand`] it means, the structural picks as ordinary intents.
    ///
    /// Opening is [`Self::open_on`]'s, called after the node draw that saw the
    /// right-click; this only ever shows a menu already latched.
    pub(crate) fn apply(&mut self, ui: &mut Ui, ctx: CanvasCtx<'_>, out: &mut Requests) {
        let graph_ctx = ctx.graph_ctx();
        self.menu.retain(|node_id| graph_ctx.contains(node_id));
        let pick = self.menu.show(ui, "node_body_menu", |ui, popup, node_id| {
            let mut chosen = None;
            // "Run to this node" and the duplicates show only when the
            // clicked node is a graph node, not a stub (same rule as the
            // header play chip): a stub neither runs nor copies. The body
            // only runs while the menu is open.
            if graph_ctx.node(node_id).is_some_and(NodeCtx::runnable) {
                if MenuItem::new("Run to this node")
                    .show(ui, popup)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Run);
                }
                MenuSeparator::new().show(ui);
                if MenuItem::new("Duplicate").show(ui, popup).left.clicked() {
                    chosen = Some(MenuChoice::Duplicate {
                        incoming: IncomingWires::Drop,
                    });
                }
                if MenuItem::new("Duplicate with incoming wires")
                    .show(ui, popup)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Duplicate {
                        incoming: IncomingWires::Keep,
                    });
                }
                MenuSeparator::new().show(ui);
            }
            if MenuItem::new("Remove").show(ui, popup).left.clicked() {
                chosen = Some(MenuChoice::Remove);
            }
            chosen
        });
        let Some(pick) = pick else {
            return;
        };
        match pick.choice {
            MenuChoice::Run => out.push_app(AppCommand::Run(RunCommand::Node(pick.target))),
            MenuChoice::Duplicate { incoming } => {
                out.extend_graph(GraphIntent::duplicate(
                    graph_ctx.document(),
                    graph_ctx.library(),
                    incoming,
                    || NodeId::unique().as_u128(),
                ));
            }
            // One intent per member, batched into a single undo entry by the
            // drain — the Delete chord's path exactly.
            MenuChoice::Remove => out.extend_graph(GraphIntent::remove_selection(graph_ctx.view())),
        }
    }
}

#[cfg(test)]
mod tests;
