use std::collections::BTreeSet;

use palantir::{MenuItem, Ui};

use crate::core::edit::intent::duplicate::build_duplicate_intent;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::paint::anchored_menu::NodeContextMenu;

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
    /// Returns the command a pick resolves to, if any — the canvas decides
    /// whether it wins the frame. A structural pick lands on `out` as
    /// ordinary intents instead, and yields `None`.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        cx: CanvasCtx<'_>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        let graph_ctx = cx.graph_ctx();
        // Boundary interface nodes carry no structural identity to
        // duplicate/remove — the sweep applies that guard, so a boundary
        // node never surfaces here.
        let opened = self.menu.latch(ui, cx);
        // Right-click selects the clicked node when it isn't already part of
        // the selection, so the chosen action always targets a coherent set
        // ("select then act"). A pick lands frames later — the menu has to
        // record at least once before an item can be clicked — so this
        // selection is committed by the time the arms below read it back.
        if let Some(node_id) = opened.filter(|&id| !graph_ctx.is_selected(id)) {
            out.push(GraphIntent::SetSelection {
                to: BTreeSet::from([node_id]),
            });
        }

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
        })?;
        // `NodeContextMenu::show` answers `Some` only for the pane that opened
        // the menu, so everything below is scoped to that pane.
        match pick.choice {
            MenuChoice::Run => Some(AppCommand::Run(RunCommand::Node(pick.node_id))),
            MenuChoice::Duplicate { incoming } => {
                out.extend(build_duplicate_intent(graph_ctx.document(), incoming));
                None
            }
            // One intent per member, batched into a single undo entry by the
            // drain — the Delete chord's path exactly.
            MenuChoice::Remove => {
                out.push_node_removals(graph_ctx.selected().iter().copied());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
