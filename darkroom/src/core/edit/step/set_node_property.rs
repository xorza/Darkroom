//! Flipping one scalar property of a node.

use scenarium::{CacheMode, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// One node property, before and after, in a variant of the one property it is about, so a
/// revert writes a disable flag over a disable flag and never a cache mode over one.
///
/// Emitted by the header badges: a sink's `D` flips `Disabled` (ambient runs
/// exclude it; an explicit node seed overrides it once); the `R`/`↓` chips
/// each flip one bit of `RuntimeCache` (the disk bit persists the output so a
/// reproducible node reloads instead of recomputing).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetNodeProperty {
    pub(crate) node_id: NodeId,
    pub(crate) property: PropertyChange,
}

/// Which property a [`SetNodeProperty`] changes, and its two values.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum PropertyChange {
    Disabled(Change<bool>),
    RuntimeCache(Change<CacheMode>),
}

impl Reversible for SetNodeProperty {
    fn write(&self, document: &mut Document, dir: Direction) {
        let node = document
            .graph
            .find_mut(self.node_id)
            .expect("history replays a property change against the node it recorded");
        match &self.property {
            PropertyChange::Disabled(change) => node.disabled = *change.half(dir),
            PropertyChange::RuntimeCache(change) => node.cache = *change.half(dir),
        }
    }

    fn is_noop(&self) -> bool {
        match &self.property {
            PropertyChange::Disabled(change) => change.unchanged(),
            PropertyChange::RuntimeCache(change) => change.unchanged(),
        }
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        EditFacts::NONE
    }
}
