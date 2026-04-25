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
    FinishedWithEmptyOutput {
        input_port: PortRef,
    },
    FinishedWithEmptyInput {
        output_port: PortRef,
    },
    FinishedWith {
        input_port: PortRef,
        output_port: PortRef,
    },
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
