//! Flipping one scalar property of a node.

use scenarium::{CacheMode, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// One scalar node property an editor can toggle. Both variants are
/// geometry-neutral — changing one never remeasures the node or reshapes a
/// graph interface — and both dirty the document, so they share one step
/// rather than taking one each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NodeProperty {
    /// `Node::disabled` — excluded from execution unless explicitly seeded.
    Disabled(bool),
    /// `Node::cache` — where the node's output is cached (see [`CacheMode`]).
    RuntimeCache(CacheMode),
}

/// One node property, before and after. Both halves name the *same* variant:
/// the capture reads back whichever property the edit is about, so a revert
/// writes a disable flag over a disable flag and never a cache mode over one.
///
/// Emitted by the header badges: a sink's `D` flips `Disabled` (ambient runs
/// exclude it; an explicit node seed overrides it once); the `R`/`↓` chips
/// each flip one bit of `RuntimeCache` (the disk bit persists the output so a
/// reproducible node reloads instead of recomputing).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetNodeProperty {
    pub(crate) node_id: NodeId,
    pub(crate) property: Change<NodeProperty>,
}

impl Reversible for SetNodeProperty {
    fn write(&self, doc: &mut Document, dir: Direction) {
        let node = doc.graph.find_mut(self.node_id).unwrap();
        match *self.property.half(dir) {
            NodeProperty::Disabled(disabled) => node.disabled = disabled,
            NodeProperty::RuntimeCache(cache) => node.cache = cache,
        }
    }

    fn is_noop(&self) -> bool {
        self.property.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    /// Disabling dims the body and a cache toggle flips a badge fill; the rect
    /// is the same either way.
    fn invalidates_cached_geometry(&self) -> bool {
        false
    }
}
