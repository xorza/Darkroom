use palantir::{MenuItem, Ui};
use scenarium::graph::NodeId;
use scenarium::graph::subgraph::SubgraphRef;

use crate::core::edit::intent::Intent;
use crate::gui::app::AppCommand;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::node::header::subgraph_badge_wid;
use crate::gui::scene::Scene;

/// Right-click on a subgraph node's `S` badge → a small popup with
/// "Publish to library" and "Detach copy". Left-click still opens the
/// subgraph (handled in `emit_subgraph_opens`); only the secondary click
/// reaches here. The open is latched off *last* frame's badge response;
/// the shared [`AnchoredMenu`] handles the popup lifecycle.
#[derive(Default, Debug)]
pub(crate) struct SubgraphMenuUi {
    menu: AnchoredMenu,
    /// Badge node the open menu targets — set at open, read at pick.
    node_id: Option<NodeId>,
}

impl SubgraphMenuUi {
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        out: &mut Vec<Intent>,
        cmd: &mut Option<AppCommand>,
    ) {
        // Latch on a secondary-click of any local-subgraph node's badge,
        // read from last frame's response (same timing as the open).
        for n in &scene.nodes {
            if matches!(n.subgraph, Some(SubgraphRef::Local(_)))
                && ui.response_for(subgraph_badge_wid(n.id)).secondary_clicked
                && let Some(p) = ui.pointer_pos()
            {
                self.node_id = Some(n.id);
                self.menu.open_at(p);
            }
        }

        let pick = self.menu.show(ui, "subgraph_node_menu", None, |ui, popup| {
            let mut chosen = None;
            if MenuItem::new("Publish to library")
                .show(ui, popup)
                .clicked()
            {
                chosen = Some(MenuChoice::Publish);
            }
            if MenuItem::new("Detach copy").show(ui, popup).clicked() {
                chosen = Some(MenuChoice::Detach);
            }
            chosen
        });
        // A pick only fires while the menu is open, where `node_id` holds
        // this open's target.
        if let Some(choice) = pick
            && let Some(node_id) = self.node_id
        {
            match choice {
                MenuChoice::Publish => {
                    *cmd = Some(AppCommand::PublishNodeSubgraph { node_id });
                }
                MenuChoice::Detach => out.push(Intent::DetachSubgraph { node_id }),
            }
        }
    }
}

enum MenuChoice {
    Publish,
    Detach,
}
