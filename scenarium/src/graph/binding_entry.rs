//! [`BindingEntry`]: one wire as a value.

use ::serde::{Deserialize, Serialize};

use crate::graph::binding::Binding;
use crate::graph::identity::InputPort;

/// One wire as a value: the consumer port, and what it was bound to.
///
/// The unit a [`DetachedNode`](crate::graph::detached::DetachedNode) collects severed wiring in.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingEntry {
    pub port: InputPort,
    pub binding: Binding,
}
