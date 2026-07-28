use palantir::{MenuItem, Ui};

use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::graph::GraphCommand;
use crate::gui::canvas::anchored_menu::NodeContextMenu;
use crate::gui::canvas::hits::{CanvasHits, MenuTrigger};
use crate::gui::scene::GraphScene;

/// Right-click on a graph node's `G` badge → a small popup with
/// "Publish to library" and "Detach copy". Left-click still opens the
/// graph (handled in `emit_graph_opens`); only the secondary click
/// reaches here. The trigger scan, the per-open node latch, and the popup
/// lifecycle are all [`NodeContextMenu`]'s.
#[derive(Default, Debug)]
pub(super) struct GraphMenuUi {
    menu: NodeContextMenu,
}

impl GraphMenuUi {
    /// Returns the command a pick resolves to, if any — the canvas decides
    /// whether it wins the frame. A "Detach copy" pick is an intent, not a
    /// command, so it lands in `out` and yields `None`.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        hits: &CanvasHits,
        graph: GraphScene<'_>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        self.menu.latch(ui, hits, graph, MenuTrigger::GraphBadge);
        let pick = self
            .menu
            .show(ui, graph, "graph_node_menu", |ui, popup, _| {
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
            })?;
        match pick.choice {
            MenuChoice::Publish => Some(AppCommand::Graph(GraphCommand::PublishGraphToLibrary {
                node_id: pick.node_id,
            })),
            MenuChoice::Detach => {
                out.push(
                    graph.target(),
                    Intent::DetachGraph {
                        node_id: pick.node_id,
                    },
                );
                None
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum MenuChoice {
    Publish,
    Detach,
}
