use std::collections::BTreeSet;

use aperture::{MenuItem, Ui};

use crate::core::edit::intent::Intent;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::node::node_widget_id;
use crate::gui::scene::Scene;

/// Right-click on a node body → a small popup with structural actions on
/// the node (and, when it's part of a multi-selection, the whole set).
/// The open is latched off *last* frame's node-body response; the shared
/// [`AnchoredMenu`] handles the popup lifecycle. Picking an action stashes a
/// [`NodeMenuAction`] the `Editor` resolves against the live selection
/// (where the `Document` is available to build the clone / removal intents).
#[derive(Default, Debug)]
pub(crate) struct NodeMenuUi {
    menu: AnchoredMenu,
    action: Option<NodeMenuAction>,
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

impl NodeMenuUi {
    pub(crate) fn apply(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        // Latch on a secondary-click of any node body (boundary interface
        // nodes carry no structural identity to duplicate/remove, so they're
        // skipped), read from last frame's response. Right-click selects the
        // clicked node when it isn't already part of the selection, so the
        // chosen action always targets a coherent set ("select then act").
        for n in &scene.nodes {
            if !n.boundary
                && ui.response_for(node_widget_id(n.id)).secondary_clicked
                && let Some(p) = ui.pointer_pos()
            {
                if !scene.selected_nodes.contains(&n.id) {
                    out.push(Intent::SetSelection {
                        to: BTreeSet::from([n.id]),
                    });
                }
                self.menu.open_at(p);
            }
        }

        let pick = self.menu.show(ui, "node_body_menu", None, |ui, popup| {
            let mut chosen = None;
            if MenuItem::new("Duplicate").show(ui, popup).clicked() {
                chosen = Some(NodeMenuAction::Duplicate);
            }
            if MenuItem::new("Duplicate with incoming connections")
                .show(ui, popup)
                .clicked()
            {
                chosen = Some(NodeMenuAction::DuplicateWithIncoming);
            }
            MenuItem::separator(ui);
            if MenuItem::new("Remove").show(ui, popup).clicked() {
                chosen = Some(NodeMenuAction::Remove);
            }
            chosen
        });
        if let Some(action) = pick {
            self.action = Some(action);
        }
    }

    /// Take the action picked since the last call, if any. The `Editor`
    /// drains this each frame and resolves it against the live selection.
    pub(crate) fn take_action(&mut self) -> Option<NodeMenuAction> {
        self.action.take()
    }
}
