use std::collections::BTreeSet;

use palantir::{MenuItem, Ui};

use crate::core::document::GraphRef;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::canvas::anchored_menu::NodeContextMenu;
use crate::gui::canvas::hits::{CanvasHits, MenuTrigger};
use crate::gui::scene::GraphScene;

/// Right-click on a node body → a small popup with actions on the node.
/// The trigger scan, the per-open node latch, and the popup lifecycle are all
/// [`NodeContextMenu`]'s. "Run to this node" resolves here (it only needs the
/// clicked node's id) and surfaces an [`AppCommand`]; the structural picks
/// stash a [`NodeMenuAction`] the `Editor` resolves against the live selection
/// (where the `Document` is available to build the clone / removal intents).
#[derive(Default, Debug)]
pub(super) struct NodeMenuUi {
    menu: NodeContextMenu,
    /// The pick and the pane it was made in. The pane travels with it
    /// because the `Editor` resolves the action against *that* graph's
    /// selection — reading the focused target instead would act on another
    /// pane whenever the menu and the focus disagree.
    action: Option<(NodeMenuAction, GraphRef)>,
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

/// A menu pick before routing: the run resolves in place (into `cmd`) against
/// the node the menu opened on, structural actions drain through
/// [`NodeMenuUi::take_action`].
#[derive(Copy, Clone, Debug)]
enum MenuChoice {
    Run,
    Action(NodeMenuAction),
}

impl NodeMenuUi {
    /// Returns the command a pick resolves to, if any — the canvas decides
    /// whether it wins the frame. A structural pick is stashed for the
    /// `Editor` instead (see [`Self::take_action`]) and yields `None`.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        hits: &CanvasHits,
        graph: GraphScene<'_>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        // Boundary interface nodes carry no structural identity to
        // duplicate/remove — the sweep applies that guard, so a boundary
        // node never surfaces here.
        let opened = self.menu.latch(ui, hits, graph, MenuTrigger::Body);
        // Right-click selects the clicked node when it isn't already part of
        // the selection, so the chosen action always targets a coherent set
        // ("select then act").
        if let Some(node_id) = opened.filter(|&id| !graph.is_selected(id)) {
            out.push(
                graph.target(),
                Intent::SetSelection {
                    to: BTreeSet::from([node_id]),
                },
            );
        }

        let pick = self
            .menu
            .show(ui, graph, "node_body_menu", |ui, popup, node_id| {
                let mut chosen = None;
                // "Run to this node" shows only when the clicked node can be a
                // run seed (same rule as the header play chip). The body only
                // runs while the menu is open.
                if graph.node(node_id).is_some_and(|n| graph.runnable(n)) {
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
                    chosen = Some(MenuChoice::Action(NodeMenuAction::Duplicate));
                }
                if MenuItem::new("Duplicate with incoming connections")
                    .show(ui, popup)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Action(NodeMenuAction::DuplicateWithIncoming));
                }
                MenuItem::separator().show(ui);
                if MenuItem::new("Remove").show(ui, popup).left.clicked() {
                    chosen = Some(MenuChoice::Action(NodeMenuAction::Remove));
                }
                chosen
            })?;
        match pick.choice {
            MenuChoice::Run => Some(AppCommand::Run(RunCommand::Node(pick.node_id))),
            MenuChoice::Action(action) => {
                // `NodeContextMenu::show` answers `Some` only for the pane that
                // opened the menu, so this is that pane.
                self.action = Some((action, graph.target()));
                None
            }
        }
    }

    /// Take the structural action picked since the last call, with the pane
    /// it was picked in. The `Editor` drains this each frame and resolves it
    /// against that pane's live selection.
    pub(super) fn take_action(&mut self) -> Option<(NodeMenuAction, GraphRef)> {
        self.action.take()
    }
}
