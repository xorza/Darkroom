use palantir::{MenuItem, MenuSeparator, Ui};

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use scenarium::{Boundary, Node, NodeId, ProgramId};

use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::gesture::item_menu::ItemMenu;

/// Right-click on a node body → a small popup with actions on the node. The trigger scan, the
/// per-open node latch, and the popup lifecycle are [`ItemMenu`]'s. "Run to this node" needs
/// only the clicked node's id and surfaces an [`AppCommand`]; the structural picks read the live
/// selection off the context and push their intents onto the frame's queue, through the same
/// builders the Ctrl+D and Delete chords drive.
#[derive(Default, Debug)]
pub(crate) struct NodeMenuUi {
    menu: ItemMenu<NodeId>,
}

/// A menu pick before routing. `Run` names the program node the menu opened on, in the instance
/// the pane shows; the structural picks
/// carry only their kind, as their target is the selection (a right-click selects the node it
/// landed on first).
#[derive(Copy, Clone, Debug)]
enum MenuChoice {
    Run {
        program_id: ProgramId,
    },
    Duplicate {
        incoming: IncomingWires,
    },
    /// Put another node in this one's place, picked from the palette.
    Replace,
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

    /// Record the menu and resolve this frame's pick onto the stage's requests — a run as
    /// the [`AppCommand`] it means, the structural picks as ordinary intents. Returns the node a
    /// Replace with… pick names, for the palette to open on.
    ///
    /// Opening is [`Self::open_on`]'s; this only ever shows a menu already latched.
    pub(crate) fn record(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, GraphCtx<'_>>,
    ) -> Option<NodeId> {
        let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
        self.menu.retain(|node_id| graph_ctx.contains(node_id));
        let pick = self.menu.show(ui, "node_body_menu", |ui, node_id| {
            let mut chosen = None;
            // The duplicates show only when the clicked node is a graph node, not a ghost node: a ghost node
            // neither runs nor copies. "Run to this node" shows by the header play chip's rule.
            if let Some(node) = graph_ctx.node(node_id).filter(|node| !node.ghost()) {
                if let Some(program_id) = node.runnable() {
                    if MenuItem::new("Run to this node").show(ui).left.clicked() {
                        chosen = Some(MenuChoice::Run { program_id });
                    }
                    MenuSeparator::new().show(ui);
                }
                if MenuItem::new("Duplicate").show(ui).clicked() {
                    chosen = Some(MenuChoice::Duplicate {
                        incoming: IncomingWires::Drop,
                    });
                }
                if MenuItem::new("Duplicate with incoming wires")
                    .show(ui)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Duplicate {
                        incoming: IncomingWires::Keep,
                    });
                }
                MenuSeparator::new().show(ui);
            }
            let boundary = graph_ctx.body().find(node_id).and_then(Node::boundary);
            // Any node with a body, an instance or a ghost node takes another in its place.
            if boundary.is_none() && MenuItem::new("Replace with\u{2026}").show(ui).clicked() {
                chosen = Some(MenuChoice::Replace);
            }
            // A compound keeps its `Outputs` node and one `Inputs` node.
            let inputs_count = || {
                graph_ctx
                    .body()
                    .iter()
                    .filter(|node| node.boundary() == Some(Boundary::Inputs))
                    .count()
            };
            let removable = match boundary {
                None => true,
                Some(Boundary::Outputs) => false,
                Some(Boundary::Inputs) => inputs_count() > 1,
            };
            if removable && MenuItem::new("Remove").show(ui).clicked() {
                chosen = Some(MenuChoice::Remove);
            }
            chosen
        });
        let pick = pick?;
        match pick.choice {
            MenuChoice::Run { program_id } => {
                out.push_app(AppCommand::Run(RunCommand::Node(program_id)));
            }
            MenuChoice::Duplicate { incoming } => {
                out.extend_graph(
                    graph_ctx.graph(),
                    GraphIntent::duplicate(
                        graph_ctx.document(),
                        graph_ctx.graph(),
                        graph_ctx.library(),
                        incoming,
                        || NodeId::unique().as_u128(),
                    ),
                );
            }
            // One intent per member, batched into a single undo entry by the drain, as the Delete
            // chord does.
            MenuChoice::Remove => out.extend_graph(
                graph_ctx.graph(),
                GraphIntent::remove_selection(graph_ctx.view()),
            ),
            MenuChoice::Replace => return Some(pick.target),
        }
        None
    }
}

#[cfg(test)]
mod tests;
