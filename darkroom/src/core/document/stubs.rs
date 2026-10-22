//! [`Stubs`]: the nodes a document keeps while the library cannot run them.

use std::collections::BTreeMap;
use std::collections::btree_map::Values;

use scenarium::{DetachedNode, NodeId};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Each node whose func the library does not resolve, as the record a detach
/// leaves: the node with every binding and subscription that touched it. The
/// load fills it from the graph's reconcile and attaches a record again once
/// its func resolves, so your settings and wiring outlive a library that lost
/// the func.
///
/// A stub has no ports, so no edit wires to one: its record is a frozen
/// snapshot. Saved as the list of records, in node-id order.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Stubs(BTreeMap<NodeId, DetachedNode>);

impl Stubs {
    /// Add `record`.
    ///
    /// # Panics
    ///
    /// If a stub already holds its node id.
    pub(crate) fn insert(&mut self, record: DetachedNode) {
        let prev = self.0.insert(record.node_id(), record);
        assert!(prev.is_none(), "a stub's node id is unique");
    }

    pub(crate) fn remove(&mut self, node_id: NodeId) -> Option<DetachedNode> {
        self.0.remove(&node_id)
    }

    pub(crate) fn get(&self, node_id: NodeId) -> Option<&DetachedNode> {
        self.0.get(&node_id)
    }

    pub(crate) fn contains(&self, node_id: NodeId) -> bool {
        self.0.contains_key(&node_id)
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// The records, in node-id order.
    pub(crate) fn iter(&self) -> Values<'_, NodeId, DetachedNode> {
        self.0.values()
    }

    /// Remove and return each record `take` picks, in node-id order.
    pub(crate) fn take_where(
        &mut self,
        mut take: impl FnMut(&DetachedNode) -> bool,
    ) -> Vec<DetachedNode> {
        self.0
            .extract_if(.., |_, record| take(record))
            .map(|(_, record)| record)
            .collect()
    }
}

impl<'a> IntoIterator for &'a Stubs {
    type Item = &'a DetachedNode;
    type IntoIter = Values<'a, NodeId, DetachedNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Serialize for Stubs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for Stubs {
    /// Refuses a list that repeats a node id, which no map could hold.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let records = Vec::<DetachedNode>::deserialize(deserializer)?;
        let mut stubs = BTreeMap::new();
        for record in records {
            let node_id = record.node_id();
            if stubs.insert(node_id, record).is_some() {
                return Err(D::Error::custom(format_args!(
                    "duplicate stub for node {node_id:?}"
                )));
            }
        }
        Ok(Self(stubs))
    }
}
