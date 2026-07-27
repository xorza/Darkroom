use std::collections::BTreeSet;

use palantir::{MenuItem, Ui};
use scenarium::NodeId;

use crate::core::document::GraphRef;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::node::node_widget_id;
use crate::gui::scene::GraphScene;

/// Right-click on a node body → a small popup with actions on the node.
/// The open is latched off *last* frame's node-body response; the shared
/// [`AnchoredMenu`] handles the popup lifecycle. "Run to this node" resolves
/// here (it only needs the clicked node's id) and surfaces an
/// [`AppCommand`]; the structural picks stash a [`NodeMenuAction`] the
/// `Editor` resolves against the live selection (where the `Document` is
/// available to build the clone / removal intents).
#[derive(Default, Debug)]
pub(super) struct NodeMenuUi {
    menu: AnchoredMenu,
    /// The pick and the pane it was made in. The pane travels with it
    /// because the `Editor` resolves the action against *that* graph's
    /// selection — reading the focused target instead would act on another
    /// pane whenever the menu and the focus disagree.
    action: Option<(NodeMenuAction, GraphRef)>,
    /// Node whose body opened the menu — the "Run to this node" target,
    /// which is the clicked node regardless of the selection. Set at open,
    /// read at pick (same latch as the graph menu's).
    target: Option<NodeId>,
}

/// A structural action picked from a node's context menu. The target is the
/// current selection (right-click first selects the clicked node), so the
/// action carries only its kind.
#[derive(Copy, Clone, Debug)]
pub(crate) enum NodeMenuAction {
    Duplicate,
    DuplicateWithIncoming,
    Remove,
}

/// A menu pick before routing: run resolves in place (into `cmd`) and
/// carries its target, structural actions drain through
/// [`NodeMenuUi::take_action`].
#[derive(Copy, Clone, Debug)]
enum MenuChoice {
    Run(NodeId),
    Action(NodeMenuAction),
}

impl NodeMenuUi {
    /// Returns the command a pick resolves to, if any — the canvas decides
    /// whether it wins the frame. A structural pick is stashed for the
    /// `Editor` instead (see [`Self::take_action`]) and yields `None`.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        graph: GraphScene<'_>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        // Latch on a secondary-click of any node body (boundary interface
        // nodes carry no structural identity to duplicate/remove, so they're
        // skipped), read from last frame's response. Right-click selects the
        // clicked node when it isn't already part of the selection, so the
        // chosen action always targets a coherent set ("select then act").
        for n in graph.nodes() {
            if !n.boundary
                && ui.response_for(node_widget_id(n.id)).right.clicked()
                && let Some(p) = ui.pointer_pos()
            {
                if !graph.is_selected(n.id) {
                    out.for_graph(graph.target(), |out| {
                        out.push(Intent::SetSelection {
                            to: BTreeSet::from([n.id]),
                        })
                    });
                }
                self.target = Some(n.id);
                self.menu.open_at(p, graph.target());
            }
        }

        let pick = self
            .menu
            .show(ui, graph.target(), "node_body_menu", None, |ui, popup| {
                let mut chosen = None;
                // "Run to this node" shows only when the clicked node can be a
                // run seed (same rule as the header play chip). The body only
                // runs while the menu is open.
                let run_target = self
                    .target
                    .and_then(|id| graph.node(id))
                    .filter(|n| graph.runnable(n))
                    .map(|n| n.id);
                if let Some(node_id) = run_target {
                    if MenuItem::new("Run to this node")
                        .show(ui, popup)
                        .left
                        .clicked()
                    {
                        chosen = Some(MenuChoice::Run(node_id));
                    }
                    MenuItem::separator(ui);
                }
                if MenuItem::new("Duplicate").show(ui, popup).left.clicked() {
                    chosen = Some(MenuChoice::Action(NodeMenuAction::Duplicate));
                }
                if MenuItem::new("Duplicate with incoming connections")
                    .show(ui, popup)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Action(NodeMenuAction::DuplicateWithIncoming));
                }
                MenuItem::separator(ui);
                if MenuItem::new("Remove").show(ui, popup).left.clicked() {
                    chosen = Some(MenuChoice::Action(NodeMenuAction::Remove));
                }
                chosen
            });
        match pick {
            Some(MenuChoice::Run(node_id)) => Some(AppCommand::Run(RunCommand::Node(node_id))),
            Some(MenuChoice::Action(action)) => {
                // `AnchoredMenu::show` answers `Some` only for the pane that
                // opened the menu, so this is that pane.
                self.action = Some((action, graph.target()));
                None
            }
            None => None,
        }
    }

    /// Take the structural action picked since the last call, with the pane
    /// it was picked in. The `Editor` drains this each frame and resolves it
    /// against that pane's live selection.
    pub(super) fn take_action(&mut self) -> Option<(NodeMenuAction, GraphRef)> {
        self.action.take()
    }
}
