//! [`NodeProperty`]: one scalar property of a node an edit sets.

use scenarium::CacheMode;
use serde::{Deserialize, Serialize};

/// One scalar node property an editor can toggle, and the value it sets.
/// Both are geometry-neutral — changing one never remeasures the node or
/// reshapes a graph interface — and both dirty the document, so they share
/// one intent and one step rather than taking one each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NodeProperty {
    /// `Node::disabled` — excluded from execution unless explicitly seeded.
    Disabled(bool),
    /// `Node::cache` — where the node's output is cached (see [`CacheMode`]).
    RuntimeCache(CacheMode),
}
