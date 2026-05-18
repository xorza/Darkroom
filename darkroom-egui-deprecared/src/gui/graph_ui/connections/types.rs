//! Pure data types for the connections subsystem. No rendering, no
//! action emission — see `mod.rs` (renderer) and `actions.rs` (drag
//! state machine, action builders).

use common::key_index_vec::KeyIndexKey;
use egui::Pos2;
use scenarium::graph::NodeId;

use crate::gui::graph_ui::connections::bezier::ConnectionBezier;
use crate::gui::graph_ui::port::{PortInfo, PortRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionKey {
    Input {
        input_node_id: NodeId,
        input_idx: usize,
    },
    Event {
        event_node_id: NodeId,
        event_idx: usize,
        trigger_node_id: NodeId,
    },
}

/// A single item broken by the connection breaker tool.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BrokeItem {
    Connection(ConnectionKey),
    Node(NodeId),
}

/// In-flight connection drag state.
///
/// Stores only identity (`PortRef`s) and the current cursor position —
/// port centers are recomputed from the current `GraphLayout` at render
/// time. The `current_pos` is initialized from the start port's center
/// on construction and kept in sync with the pointer + snap targets by
/// `advance_drag`.
#[derive(Debug)]
pub(crate) struct ConnectionDrag {
    pub(crate) start_port: PortRef,
    pub(crate) end_port: Option<PortRef>,
    pub(crate) current_pos: Pos2,
}

impl ConnectionDrag {
    pub(crate) fn new(start: PortInfo) -> Self {
        Self {
            current_pos: start.center,
            start_port: start.port,
            end_port: None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ConnectionDragUpdate {
    InProgress,
    Finished,
    FinishedWithEmptyOutput { input_port: PortRef },
    FinishedWithEmptyInput { output_port: PortRef },
    FinishedWith(ConnectionPair),
}

/// A snapped pair of compatible ports, classified by connection kind.
/// Constructed only via [`super::actions::pair_ports`]; downstream code
/// matches on the variant without re-checking [`crate::gui::graph_ui::port::PortKind`],
/// so impossible cross-kind pairs cannot be expressed at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionPair {
    Data { input: PortRef, output: PortRef },
    Event { trigger: PortRef, event: PortRef },
}

impl ConnectionPair {
    /// The `ConnectionKey` that addresses the (potential) committed
    /// connection — the same id used by the permanent curve so the
    /// in-flight temp curve and the committed curve share an egui id.
    pub(crate) fn key(&self) -> ConnectionKey {
        match *self {
            ConnectionPair::Data { input, .. } => ConnectionKey::Input {
                input_node_id: input.node_id,
                input_idx: input.port_idx,
            },
            ConnectionPair::Event { trigger, event } => ConnectionKey::Event {
                event_node_id: event.node_id,
                event_idx: event.port_idx,
                trigger_node_id: trigger.node_id,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct ConnectionCurve {
    pub(crate) key: ConnectionKey,
    pub(crate) broke: bool,
    pub(crate) hovered: bool,
    pub(crate) bezier: ConnectionBezier,
}

impl ConnectionCurve {
    pub(crate) fn new(key: ConnectionKey) -> Self {
        Self {
            key,
            broke: false,
            hovered: false,
            bezier: ConnectionBezier::default(),
        }
    }
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}
