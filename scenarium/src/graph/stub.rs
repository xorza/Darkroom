//! [`Stub`]: a node its graph cannot hold, kept with its wiring and why.
//!
//! # Decisions
//!
//! - **Why a node was detached is kept with its stub**, apart from why a node does not resolve: a
//!   node detached for a rule across graphs, a loop or a bound, resolves, so the reason cannot be
//!   read off it again. Every stub whose node resolves is attached at the next load, and the load
//!   decides again.

use ::serde::{Deserialize, Serialize};

use crate::graph::detached::DetachedNode;
use crate::graph::error::DetachReason;
use crate::graph::identity::NodeId;

/// A node its graph cannot hold, as the record a detach leaves — the node with every binding and
/// subscription that touched it — and why the load took it out. Kept in the graph beside its
/// nodes, so your settings and wiring outlive a library that lost a func, and a definition is one
/// value wherever it goes. A stub has no ports, so no edit wires to one: its record is a frozen
/// snapshot until a load attaches it again.
///
/// The reason is kept, not read off the node again: a node detached for a rule across graphs, a
/// loop or a bound, resolves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stub {
    pub record: DetachedNode,
    pub reason: DetachReason,
}

impl Stub {
    pub const fn node_id(&self) -> NodeId {
        self.record.node_id()
    }
}
