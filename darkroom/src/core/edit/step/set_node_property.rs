//! Flipping one scalar property of a node.

use scenarium::{CacheMode, CompoundId, GraphId, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// One node property, before and after, in a variant of the one property it is about, so a
/// revert writes a disable flag over a disable flag and never a cache mode over one.
///
/// Emitted by the header badges: the disable badge flips `Disabled`, the cache chips each flip one
/// bit of `RuntimeCache`.
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
    Compound(Change<CompoundId>),
}

impl Reversible for SetNodeProperty {
    fn write(&self, document: &mut Document, graph: GraphId, dir: Direction) {
        let node = document
            .slot_mut(graph)
            .graph
            .find_mut(self.node_id)
            .expect("history replays a property change against the node it recorded");
        match &self.property {
            PropertyChange::Disabled(change) => node.disabled = *change.half(dir),
            PropertyChange::RuntimeCache(change) => {
                *node
                    .cache_mut()
                    .expect("a node whose cache mode history recorded has one") = *change.half(dir);
            }
            PropertyChange::Compound(change) => node.set_compound(*change.half(dir)),
        }
    }

    fn is_noop(&self) -> bool {
        match &self.property {
            PropertyChange::Disabled(change) => change.unchanged(),
            PropertyChange::RuntimeCache(change) => change.unchanged(),
            PropertyChange::Compound(change) => change.unchanged(),
        }
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        EditFacts::NONE
    }
}
