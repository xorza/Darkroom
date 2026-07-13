//! The group-drag anchor shape shared by a node-body drag
//! ([`crate::gui::node::NodeUI`]'s `DragAnchor`) and a pin-widget drag
//! ([`crate::gui::canvas::pin_ui::PinUi`]'s `PinDragAnchor`): whichever
//! member the pointer latched drags its whole group (every other selected
//! node and pin) alongside it. Every later frame's committed position is
//! `start + drag_delta`, not a running integration over the moving widget,
//! so both draw a fresh `Intent::MoveSelection` off the same start snapshot
//! each frame the drag is held.

use glam::Vec2;
use scenarium::graph::{NodeId, OutputPort};

use crate::core::document::SelectionKey;
use crate::core::edit::intent::types::Intent;

/// `K` is the grabbed member's own key — a `NodeId` for a node body, an
/// `OutputPort` for a pin widget — kept generic so each caller's `apply`/
/// `prepass` can pattern-match its own domain type without downcasting a
/// shared enum.
#[derive(Clone, Debug)]
pub(crate) struct GroupDragAnchor<K> {
    pub(crate) key: K,
    /// Every node moving with this drag and its position at drag start:
    /// the whole selection's nodes when the grabbed member was already
    /// selected, else just the grabbed node (empty for a lone pin drag).
    pub(crate) start_node_positions: Vec<(NodeId, Vec2)>,
    /// Every pinned-output preview moving with this drag and its absolute
    /// position at drag start, on the same "whole selection, or just the
    /// grabbed member" rule.
    pub(crate) start_pin_positions: Vec<(OutputPort, Vec2)>,
    /// Captured at latch so later frames can `ui.response_for(widget_id)`
    /// directly: the node body/title (or port circle / preview widget for
    /// a pin) whose drag delta drives this anchor.
    pub(crate) widget_id: aperture::WidgetId,
}

impl<K> GroupDragAnchor<K> {
    /// Rebuild this frame's `Intent::MoveSelection` from a zoom-adjusted
    /// `offset` — the "start + offset" shape both callers' resolve step
    /// shares, regardless of what kind of member grabbed the drag.
    /// `grabbed` names the member for the intent's own record (the caller
    /// already knows which `SelectionKey` variant wraps `self.key`).
    pub(crate) fn resolve(&self, offset: Vec2, grabbed: SelectionKey) -> Intent {
        let nodes = self
            .start_node_positions
            .iter()
            .map(|(id, start)| (*id, *start + offset))
            .collect();
        let pins = self
            .start_pin_positions
            .iter()
            .map(|(port, start)| (*port, *start + offset))
            .collect();
        Intent::MoveSelection {
            grabbed,
            nodes,
            pins,
        }
    }
}
