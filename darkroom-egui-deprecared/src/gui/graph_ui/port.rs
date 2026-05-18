//! Port primitives shared by node rendering, connection rendering, and the
//! gesture state machine. Kept in its own module so they have a single home
//! — historically `PortKind` lived in `connection_ui.rs` and `PortRef`
//! lived in `graph_layout.rs`, which made imports cross unrelated files
//! for one concept.

use egui::Pos2;
use scenarium::graph::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,
    Trigger,
    Event,
}

impl PortKind {
    pub fn opposite(&self) -> Self {
        match self {
            PortKind::Input => PortKind::Output,
            PortKind::Output => PortKind::Input,
            PortKind::Trigger => PortKind::Event,
            PortKind::Event => PortKind::Trigger,
        }
    }

    pub(crate) fn is_source(&self) -> bool {
        matches!(self, PortKind::Output | PortKind::Event)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRef {
    pub node_id: NodeId,
    pub port_idx: usize,
    pub kind: PortKind,
}

#[derive(Debug, Clone, Copy)]
pub struct PortInfo {
    pub port: PortRef,
    pub center: Pos2,
}
