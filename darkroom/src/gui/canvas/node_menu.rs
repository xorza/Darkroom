use std::collections::BTreeSet;

use glam::Vec2;
use palantir::{ClickOutside, Configure, MenuItem, Popup, Sizing, Spacing, Ui};

use crate::core::edit::intent::Intent;
use crate::gui::node::node_widget_id;
use crate::gui::scene::Scene;

/// Right-click on a node body → a small popup with structural actions on
/// the node (and, when it's part of a multi-selection, the whole set).
/// Modeled on [`crate::gui::canvas::subgraph_menu`]: the open is latched
/// off *last* frame's node-body response, then the popup is shown and its
/// clicks handled this frame. Picking an action stashes a [`NodeMenuAction`]
/// the `Editor` resolves against the live selection (where the `Document`
/// is available to build the clone / removal intents).
#[derive(Default, Debug)]
pub(crate) struct NodeMenuUi {
    open: Option<OpenState>,
    action: Option<NodeMenuAction>,
}

#[derive(Copy, Clone, Debug)]
struct OpenState {
    /// Surface-space anchor for [`Popup::anchored_to`].
    anchor: Vec2,
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
                self.open = Some(OpenState { anchor: p });
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
        let mut chosen: Option<NodeMenuAction> = None;
        let popup_resp = Popup::anchored_to(open.anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt("node_body_menu")
            .size((Sizing::Hug, Sizing::Hug))
            .padding(Spacing::all(6.0))
            .show(ui, |ui, popup| {
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
            });

        if let Some(action) = chosen {
            self.action = Some(action);
            self.open = None;
        } else if popup_resp.dismissed || popup_resp.close_requested {
            self.open = None;
        }
    }

    /// Take the action picked since the last call, if any. The `Editor`
    /// drains this each frame and resolves it against the live selection.
    pub(crate) fn take_action(&mut self) -> Option<NodeMenuAction> {
        self.action.take()
    }
}
