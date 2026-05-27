use glam::Vec2;
use palantir::{ClickOutside, Configure, MenuItem, Popup, Sizing, Spacing, Ui};
use scenarium::prelude::NodeId;
use scenarium::subgraph::SubgraphRef;

use crate::edit::intent::Intent;
use crate::gui::menu_bar::MenuCommand;
use crate::gui::node::header::subgraph_badge_wid;
use crate::scene::Scene;

/// Right-click on a subgraph node's `S` badge → a small popup with
/// "Publish to library" and "Detach copy". Left-click still opens the
/// subgraph (handled in `emit_subgraph_opens`); only the secondary
/// click reaches here. Modeled on [`crate::gui::canvas::new_node_ui`]:
/// the open is latched off *last* frame's badge response, then the
/// popup is shown and its clicks handled this frame.
#[derive(Default, Debug)]
pub(crate) struct SubgraphMenuUi {
    open: Option<OpenState>,
}

#[derive(Copy, Clone, Debug)]
struct OpenState {
    node_id: NodeId,
    /// Surface-space anchor for [`Popup::anchored_to`].
    anchor: Vec2,
}

impl SubgraphMenuUi {
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        out: &mut Vec<Intent>,
        cmd: &mut Option<MenuCommand>,
    ) {
        // Latch on a secondary-click of any local-subgraph node's badge,
        // read from last frame's response (same timing as the open).
        for n in &scene.nodes {
            if matches!(n.subgraph, Some(SubgraphRef::Local(_)))
                && ui.response_for(subgraph_badge_wid(n.id)).secondary_clicked
                && let Some(p) = ui.pointer_pos()
            {
                self.open = Some(OpenState {
                    node_id: n.id,
                    anchor: p,
                });
            }
        }

        let Some(open) = self.open else {
            return;
        };
        if ui.escape_pressed() {
            self.open = None;
            return;
        }

        let chrome = ui.theme.context_menu.panel.clone();
        let mut chosen: Option<MenuChoice> = None;
        let popup_resp = Popup::anchored_to(open.anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt("subgraph_node_menu")
            .size((Sizing::Hug, Sizing::Hug))
            .padding(Spacing::all(6.0))
            .show(ui, |ui, popup| {
                if MenuItem::new("Publish to library")
                    .show(ui, popup)
                    .clicked()
                {
                    chosen = Some(MenuChoice::Publish);
                }
                if MenuItem::new("Detach copy").show(ui, popup).clicked() {
                    chosen = Some(MenuChoice::Detach);
                }
            });

        match chosen {
            Some(MenuChoice::Publish) => {
                *cmd = Some(MenuCommand::PublishNodeSubgraph {
                    node_id: open.node_id,
                });
                self.open = None;
            }
            Some(MenuChoice::Detach) => {
                out.push(Intent::DetachSubgraph {
                    node_id: open.node_id,
                });
                self.open = None;
            }
            None => {
                if popup_resp.dismissed || popup_resp.close_requested {
                    self.open = None;
                }
            }
        }
    }
}

enum MenuChoice {
    Publish,
    Detach,
}
