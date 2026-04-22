//! Graph UI pointer-gesture state machine.
//!
//! The user is always doing exactly one thing in the graph editor. Each
//! [`Gesture`] variant carries the data that thing needs — so the
//! breaker only exists while the user is breaking connections, and the
//! in-flight connection drag only exists while the user is dragging a new
//! one. There is no separate "stop drag" / "reset breaker" dance:
//! transitions replace the variant atomically.

use egui::{Pos2, Vec2};
use scenarium::graph::NodeId;

use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::ConnectionDrag;
use crate::gui::graph_layout::PortInfo;

/// In-flight node drag. `start_pos` is the node's position at drag start
/// (used as the `before` of the emitted `NodeMoved` action); `offset`
/// accumulates per-frame drag deltas. Render composes
/// `view_node.pos + offset` to show the dragged position; `ViewGraph`
/// itself is untouched until the drag commits on release.
#[derive(Debug, Clone, Copy)]
pub struct NodeDrag {
    pub node_id: NodeId,
    pub start_pos: Pos2,
    pub offset: Vec2,
    /// Set on the frame the user releases the mouse: the `NodeMoved`
    /// action has been emitted, but we must keep the offset alive
    /// through this frame's render so the node doesn't flash back to
    /// `view_node.pos` (old) before `apply()` updates it at end of
    /// frame. Cleared + gesture cancelled at the start of the next
    /// frame's interaction pass.
    pub released: bool,
}

impl NodeDrag {
    pub fn committed_pos(&self) -> Pos2 {
        self.start_pos + self.offset
    }
}

#[derive(Debug, Default)]
pub enum Gesture {
    #[default]
    Idle,
    Panning,
    DraggingConnection(ConnectionDrag),
    BreakingConnections(ConnectionBreaker),
    DraggingNode(NodeDrag),
}

impl Gesture {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_panning(&self) -> bool {
        matches!(self, Self::Panning)
    }

    pub fn is_breaking(&self) -> bool {
        matches!(self, Self::BreakingConnections(_))
    }

    pub fn is_dragging_connection(&self) -> bool {
        matches!(self, Self::DraggingConnection(_))
    }

    pub fn is_dragging_node(&self) -> bool {
        matches!(self, Self::DraggingNode(_))
    }

    /// Cancels whatever the user was doing. One assignment — there is no
    /// partial state left behind.
    pub fn cancel(&mut self) {
        *self = Self::Idle;
    }

    pub fn start_panning(&mut self) {
        *self = Self::Panning;
    }

    pub fn start_dragging(&mut self, start_port: PortInfo) {
        *self = Self::DraggingConnection(ConnectionDrag::new(start_port));
    }

    pub fn start_breaking(&mut self, start_pos: Pos2) {
        let mut breaker = ConnectionBreaker::default();
        breaker.start(start_pos);
        *self = Self::BreakingConnections(breaker);
    }

    pub fn start_node_drag(&mut self, node_id: NodeId, start_pos: Pos2) {
        *self = Self::DraggingNode(NodeDrag {
            node_id,
            start_pos,
            offset: Vec2::ZERO,
            released: false,
        });
    }

    pub fn node_drag(&self) -> Option<&NodeDrag> {
        if let Self::DraggingNode(d) = self {
            Some(d)
        } else {
            None
        }
    }

    pub fn node_drag_mut(&mut self) -> Option<&mut NodeDrag> {
        if let Self::DraggingNode(d) = self {
            Some(d)
        } else {
            None
        }
    }

    /// Per-node drag offset, or `Vec2::ZERO` when the node isn't being
    /// dragged. Render uses this to compose an effective position on top
    /// of `view_node.pos` without touching the graph.
    pub fn node_drag_offset_for(&self, node_id: &NodeId) -> Vec2 {
        self.node_drag()
            .filter(|d| d.node_id == *node_id)
            .map(|d| d.offset)
            .unwrap_or(Vec2::ZERO)
    }

    pub fn breaker(&self) -> Option<&ConnectionBreaker> {
        if let Self::BreakingConnections(b) = self {
            Some(b)
        } else {
            None
        }
    }

    pub fn breaker_mut(&mut self) -> Option<&mut ConnectionBreaker> {
        if let Self::BreakingConnections(b) = self {
            Some(b)
        } else {
            None
        }
    }

    pub fn drag(&self) -> Option<&ConnectionDrag> {
        if let Self::DraggingConnection(d) = self {
            Some(d)
        } else {
            None
        }
    }

    pub fn drag_mut(&mut self) -> Option<&mut ConnectionDrag> {
        if let Self::DraggingConnection(d) = self {
            Some(d)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::connection_ui::PortKind;
    use crate::gui::graph_layout::PortRef;
    use scenarium::graph::NodeId;

    fn dummy_port() -> PortInfo {
        PortInfo {
            port: PortRef {
                node_id: NodeId::unique(),
                kind: PortKind::Output,
                port_idx: 0,
            },
            center: Pos2::ZERO,
        }
    }

    #[test]
    fn default_is_idle() {
        let i = Gesture::default();
        assert!(i.is_idle());
        assert!(i.breaker().is_none());
        assert!(i.drag().is_none());
    }

    #[test]
    fn start_breaking_installs_breaker() {
        let mut i = Gesture::default();
        i.start_breaking(Pos2::new(10.0, 20.0));
        assert!(i.is_breaking());
        assert!(i.breaker().is_some());
        assert!(i.drag().is_none());
    }

    #[test]
    fn start_dragging_installs_drag_only() {
        let mut i = Gesture::default();
        i.start_dragging(dummy_port());
        assert!(i.is_dragging_connection());
        assert!(i.drag().is_some());
        assert!(i.breaker().is_none());
    }

    #[test]
    fn cancel_drops_variant_data() {
        let mut i = Gesture::default();
        i.start_breaking(Pos2::ZERO);
        i.cancel();
        assert!(i.is_idle());
        assert!(i.breaker().is_none());

        i.start_dragging(dummy_port());
        i.cancel();
        assert!(i.is_idle());
        assert!(i.drag().is_none());
    }

    #[test]
    fn transition_between_variants_replaces_data() {
        let mut i = Gesture::default();
        i.start_breaking(Pos2::ZERO);
        i.start_dragging(dummy_port());
        assert!(i.is_dragging_connection());
        assert!(i.breaker().is_none());

        i.start_panning();
        assert!(i.is_panning());
        assert!(i.drag().is_none());
    }
}
