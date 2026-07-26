use palantir::{MenuItem, Ui};
use scenarium::GraphLink;
use scenarium::NodeId;

use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::graph::GraphCommand;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::node::header::graph_badge_wid;
use crate::gui::scene::GraphScene;

/// Right-click on a graph node's `G` badge → a small popup with
/// "Publish to library" and "Detach copy". Left-click still opens the
/// graph (handled in `emit_graph_opens`); only the secondary click
/// reaches here. The open is latched off *last* frame's badge response;
/// the shared [`AnchoredMenu`] handles the popup lifecycle.
#[derive(Default, Debug)]
pub(super) struct GraphMenuUi {
    menu: AnchoredMenu,
    /// Badge node the open menu targets — set at open, read at pick.
    node_id: Option<NodeId>,
}

impl GraphMenuUi {
    /// Returns the command a pick resolves to, if any — the canvas decides
    /// whether it wins the frame. A "Detach copy" pick is an intent, not a
    /// command, so it lands in `out` and yields `None`.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        graph: GraphScene<'_>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        // Latch on a secondary-click of any local-graph node's badge,
        // read from last frame's response (same timing as the open).
        for n in graph.nodes() {
            if matches!(n.graph, Some(GraphLink::Local(_)))
                && ui.response_for(graph_badge_wid(n.id)).right.clicked()
                && let Some(p) = ui.pointer_pos()
            {
                self.node_id = Some(n.id);
                self.menu.open_at(p, graph.target());
            }
        }

        let pick = self
            .menu
            .show(ui, graph.target(), "graph_node_menu", None, |ui, popup| {
                let mut chosen = None;
                if MenuItem::new("Publish to library")
                    .show(ui, popup)
                    .left
                    .clicked()
                {
                    chosen = Some(MenuChoice::Publish);
                }
                if MenuItem::new("Detach copy").show(ui, popup).left.clicked() {
                    chosen = Some(MenuChoice::Detach);
                }
                chosen
            });
        // A pick only fires while the menu is open, where `node_id` holds
        // this open's target.
        let (choice, node_id) = (pick?, self.node_id?);
        match choice {
            MenuChoice::Publish => Some(AppCommand::Graph(GraphCommand::PublishGraphToLibrary {
                node_id,
            })),
            MenuChoice::Detach => {
                out.for_graph(graph.target(), |out| {
                    out.push(Intent::DetachGraph { node_id })
                });
                None
            }
        }
    }
}

#[derive(Debug)]
enum MenuChoice {
    Publish,
    Detach,
}
