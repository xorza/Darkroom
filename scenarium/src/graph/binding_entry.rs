//! [`BindingEntry`]: one data edge as a value.

use ::serde::{Deserialize, Serialize};

use crate::graph::binding::Binding;
use crate::graph::identity::InputPort;

/// One data edge as a value: the consumer port, and what it was bound to.
///
/// The unit every reversible edit collects severed wiring into: a
/// [`DetachedNode`](crate::graph::detached::DetachedNode) carries the edges that touched its node,
/// so an attach restores exactly what a detach took.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingEntry {
    pub port: InputPort,
    pub binding: Binding,
}
