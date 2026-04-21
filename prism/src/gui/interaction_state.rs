//! Graph UI interaction state machine.
//!
//! The user is always doing exactly one thing in the graph editor. Each
//! [`Interaction`] variant carries the data that thing needs — so the
//! breaker only exists while the user is breaking connections, and the
//! in-flight connection drag only exists while the user is dragging a new
//! one. There is no separate "stop drag" / "reset breaker" dance:
//! transitions replace the variant atomically.

use egui::Pos2;

use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::ConnectionDrag;
use crate::gui::graph_layout::PortInfo;

#[derive(Debug, Default)]
pub enum Interaction {
    #[default]
    Idle,
    Panning,
    DraggingConnection(ConnectionDrag),
    BreakingConnections(ConnectionBreaker),
}

impl Interaction {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_panning(&self) -> bool {
        matches!(self, Self::Panning)
    }

    pub fn is_breaking(&self) -> bool {
        matches!(self, Self::BreakingConnections(_))
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self, Self::DraggingConnection(_))
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
        let i = Interaction::default();
        assert!(i.is_idle());
        assert!(i.breaker().is_none());
        assert!(i.drag().is_none());
    }

    #[test]
    fn start_breaking_installs_breaker() {
        let mut i = Interaction::default();
        i.start_breaking(Pos2::new(10.0, 20.0));
        assert!(i.is_breaking());
        assert!(i.breaker().is_some());
        assert!(i.drag().is_none());
    }

    #[test]
    fn start_dragging_installs_drag_only() {
        let mut i = Interaction::default();
        i.start_dragging(dummy_port());
        assert!(i.is_dragging());
        assert!(i.drag().is_some());
        assert!(i.breaker().is_none());
    }

    #[test]
    fn cancel_drops_variant_data() {
        let mut i = Interaction::default();
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
        let mut i = Interaction::default();
        i.start_breaking(Pos2::ZERO);
        i.start_dragging(dummy_port());
        assert!(i.is_dragging());
        assert!(i.breaker().is_none());

        i.start_panning();
        assert!(i.is_panning());
        assert!(i.drag().is_none());
    }
}
