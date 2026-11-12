//! [`NodeProperty`]: one scalar property of a node an edit sets.

use scenarium::CacheMode;
use serde::{Deserialize, Serialize};

/// One scalar node property an editor can toggle, and the value it sets.
/// Both are geometry-neutral and both dirty the document, so they share one intent and one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NodeProperty {
    /// `Node::disabled` — excluded from execution unless explicitly seeded.
    Disabled(bool),
    /// `Node::cache` — where the node's output is cached (see [`CacheMode`]).
    RuntimeCache(CacheMode),
}
