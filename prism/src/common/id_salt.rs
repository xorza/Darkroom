//! Standardized ID salt generation for egui persistent IDs.
//!
//! This module provides consistent ID generation patterns for UI elements.
//! All functions return tuples that can be passed to `ui.make_persistent_id()`.

use scenarium::graph::NodeId;

use crate::gui::connection_ui::PortKind;

/// ID salts for node-related UI elements.
pub struct NodeIds;

impl NodeIds {
    /// ID for the node body interaction area.
    pub fn body(node_id: NodeId) -> (&'static str, NodeId) {
        ("node_body", node_id)
    }

    /// ID for storing drag start position during node dragging.
    pub fn drag_start(node_id: NodeId) -> (&'static str, NodeId) {
        ("node_drag_start", node_id)
    }

    /// ID for the impure status dot tooltip.
    pub fn status_impure(node_id: NodeId) -> (&'static str, NodeId) {
        ("node_status_impure", node_id)
    }
}

/// ID salts for port-related UI elements.
pub struct PortIds;

impl PortIds {
    /// ID for a port interaction area.
    pub fn port(
        node_id: NodeId,
        kind: PortKind,
        idx: usize,
    ) -> (&'static str, PortKind, NodeId, usize) {
        ("node_port", kind, node_id, idx)
    }
}

/// ID salts for const binding UI elements.
pub struct ConstBindIds;

impl ConstBindIds {
    /// ID for the const binding bezier connection.
    pub fn link(node_id: NodeId, input_idx: usize) -> (&'static str, NodeId, usize) {
        ("const_link", node_id, input_idx)
    }

    /// ID for the const value editor widget.
    pub fn value(node_id: NodeId, input_idx: usize) -> (&'static str, NodeId, usize) {
        ("const_value", node_id, input_idx)
    }
}
