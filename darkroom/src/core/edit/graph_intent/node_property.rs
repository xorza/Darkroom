//! [`NodeProperty`]: one scalar property of a node an edit sets.

use scenarium::{CacheMode, CompoundId};
use serde::{Deserialize, Serialize};

/// One scalar node property an editor can set, and the value it sets. Each is geometry-neutral
/// and dirties the document, so they share one intent and one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NodeProperty {
    /// `Node::disabled` — excluded from execution unless explicitly seeded.
    Disabled(bool),
    /// `Node::cache` — where the node's output is cached (see [`CacheMode`]).
    RuntimeCache(CacheMode),
    /// An instance's compound: Make Unique points the instances it picks at the copy it makes,
    /// whose interface is the same, so no port changes.
    Compound(CompoundId),
}
