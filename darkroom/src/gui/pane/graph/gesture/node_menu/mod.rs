use std::collections::BTreeSet;

use palantir::{MenuItem, Ui};

use crate::core::edit::intent::duplicate::build_duplicate_intent;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use scenarium::NodeId;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::paint::anchored_menu::NodeContextMenu;
use crate::gui::requests::Requests;

/// Right-click on a node body → a small popup with actions on the node.
/// The trigger scan, the per-open node latch, and the popup lifecycle are all
/// [`NodeContextMenu`]'s. "Run to this node" needs only the clicked node's id
/// and surfaces an [`AppCommand`]; the structural picks read the live
/// selection off the context and push their intents onto the frame's queue,
/// the same two builders the Ctrl+D and Delete chords drive.
#[derive(Default, Debug)]
pub(crate) struct NodeMenuUi {
    menu: NodeContextMenu,
}

/// A menu pick before routing. `Run` names the node the menu opened on; the
/// structural picks carry only their kind, because their target is the
/// selection (a right-click selects the node it landed on first).
#[derive(Copy, Clone, Debug)]
enum MenuChoice {
    Run,
    /// `incoming`: keep the clones' wires to producers outside the selection.
    Duplicate {
        incoming: bool,
    },
    Remove,
}

impl NodeMenuUi {
    /// Close the menu.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
    }

    /// Record the menu and resolve this frame's pick onto `out` — a run as
    /// the [`AppCommand`] it means, the structural picks as ordinary intents.
    ///
    /// Opening is [`Self::open_on`]'s, called after the node draw that saw the
    /// right-click; this only ever shows a menu already latched.
    /// Open the menu on `node`, which the record pass just saw right-clicked,
    /// and select it if it isn't already part of the selection — so the chosen
    /// action always targets a coherent set ("select then act"). A pick lands
    /// frames later, so that selection is committed well before it is read
    /// back.
    pub(crate) fn open_on(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        node: NodeId,
        out: &mut Requests,
    ) {
        if !self.menu.open_on(ui, node) {
            return;
        }
        if !graph_ctx.is_selected(node) {
            out.push_graph(GraphIntent::SetSelection {
                to: BTreeSet::from([node]),
            });
        }
    }

    pub(crate) fn apply(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        let pick = self.menu.show(ui, "node_body_menu", |ui, popup, node_id| {
            let mut chosen = None;
            // "Run to this node" shows only when the clicked node can be a
            // run seed (same rule as the header play chip). The body only
            // runs while the menu is open.
            if graph_ctx.node(node_id).is_some_and(|n| n.runnable()) {
                if MenuItem::new("Run to this node")
                    .show(ui, popup)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Run);
                }
                MenuItem::separator().show(ui);
            }
            if MenuItem::new("Duplicate").show(ui, popup).left.clicked() {
                chosen = Some(MenuChoice::Duplicate { incoming: false });
            }
            if MenuItem::new("Duplicate with incoming connections")
                .show(ui, popup)
                .left
                .clicked()
            {
                chosen = Some(MenuChoice::Duplicate { incoming: true });
            }
            MenuItem::separator().show(ui);
            if MenuItem::new("Remove").show(ui, popup).left.clicked() {
                chosen = Some(MenuChoice::Remove);
            }
            chosen
        });
        let Some(pick) = pick else {
            return;
        };
        // `NodeContextMenu::show` answers `Some` only for the pane that opened
        // the menu, so everything below is scoped to that pane.
        match pick.choice {
            MenuChoice::Run => out.push_app(AppCommand::Run(RunCommand::Node(pick.node_id))),
            MenuChoice::Duplicate { incoming } => {
                out.extend_graph(build_duplicate_intent(graph_ctx.document(), incoming));
            }
            // One intent per member, batched into a single undo entry by the
            // drain — the Delete chord's path exactly.
            MenuChoice::Remove => out.extend_graph(
                graph_ctx
                    .selected()
                    .iter()
                    .map(|&node_id| GraphIntent::RemoveNode { node_id }),
            ),
        }
    }
}

#[cfg(test)]
mod tests;
