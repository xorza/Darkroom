//! Field codecs for [`Graph`](crate::graph::Graph)'s tables and a set's subgraphs. A map built by
//! repeated `insert` keeps the last value for a repeated key, so a corrupt document would load as
//! a quietly different graph. Each codec decodes entry by entry and rejects the collision, since
//! validation could only assert after the evidence was gone.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use ::serde::de::Error as _;
use ::serde::de::{MapAccess, Visitor};
use ::serde::{Deserialize, Deserializer, Serialize, Serializer};
use hashbrown::HashMap;

use crate::graph::binding::Binding;
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{GhostBinding, GhostSubscription};
use crate::graph::identity::{FuncId, InputPort, NodeId, SubgraphId};
use crate::graph::node::Node;
use crate::graph::saved_decl::SavedDecl;
use crate::graph::subgraph::Subgraph;
use crate::graph::subscription::Subscription;
use crate::graph::type_names::TypeRef;

/// A table whose key is a struct travels as a sequence of `(key, value)` pairs.
pub(super) fn serialize_pairs<K: Serialize, V: Serialize, S: Serializer>(
    map: &BTreeMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(map)
}

/// A table of pairs, refusing a key given twice: `what` names the entry in the refusal.
fn unique_pairs<'de, D, K, V>(deserializer: D, what: &str) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord + Copy + fmt::Debug,
    V: Deserialize<'de>,
{
    let entries = Vec::<(K, V)>::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for (key, value) in entries {
        if map.insert(key, value).is_some() {
            return Err(D::Error::custom(format!("duplicate {what} {key:?}")));
        }
    }
    Ok(map)
}

pub(super) fn deserialize_bindings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<InputPort, Binding>, D::Error> {
    unique_pairs(deserializer, "binding for input port")
}

/// Refuses an input given two ghosts, as [`deserialize_bindings`] refuses two bindings.
pub(super) fn deserialize_ghost_bindings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<InputPort, GhostBinding>, D::Error> {
    unique_pairs(deserializer, "ghost binding for input port")
}

pub(super) fn deserialize_ghost_subscriptions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Subscription, GhostSubscription>, D::Error> {
    unique_pairs(deserializer, "ghost of")
}

/// The node table, decoded as a map like the derive would, but refusing a repeated id: two
/// entries under one id are two different nodes, and keeping the last silently rebinds every
/// wire naming that id. Only here do both entries still exist.
pub(super) fn deserialize_nodes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<NodeId, Node>, D::Error> {
    deserializer.deserialize_map(NodesVisitor)
}

/// The subgraph table, decoded as a map refusing a repeated id, as [`deserialize_nodes`] does
/// for nodes: two subgraphs under one id are two definitions every instance of either names.
pub(super) fn deserialize_subgraphs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<SubgraphId, Subgraph>, D::Error> {
    deserializer.deserialize_map(UniqueMapVisitor {
        what: "subgraph id",
        entry: PhantomData,
    })
}

/// The ghost nodes, decoded as a map refusing a repeated id: two ghost nodes under one id are
/// two nodes, and keeping the last loses the other's settings.
pub(super) fn deserialize_ghost_nodes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<NodeId, GhostNode>, D::Error> {
    deserializer.deserialize_map(UniqueMapVisitor {
        what: "ghost node id",
        entry: PhantomData,
    })
}

/// A manifest's funcs, decoded as a map refusing a repeated id.
pub(super) fn deserialize_manifest_funcs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<FuncId, SavedDecl>, D::Error> {
    deserializer.deserialize_map(UniqueMapVisitor {
        what: "func id",
        entry: PhantomData,
    })
}

pub(super) fn deserialize_type_names<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<TypeRef, String>, D::Error> {
    deserializer.deserialize_map(UniqueMapVisitor {
        what: "type",
        entry: PhantomData,
    })
}

/// A map in key order that refuses a key given twice, naming it as `what`.
struct UniqueMapVisitor<K, V> {
    what: &'static str,
    entry: PhantomData<fn() -> (K, V)>,
}

impl<K, V> fmt::Debug for UniqueMapVisitor<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UniqueMapVisitor({})", self.what)
    }
}

impl<'de, K, V> Visitor<'de> for UniqueMapVisitor<K, V>
where
    K: Deserialize<'de> + Ord + Copy + fmt::Debug,
    V: Deserialize<'de>,
{
    type Value = BTreeMap<K, V>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a map keyed by {}", self.what)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut map = BTreeMap::new();
        while let Some((key, value)) = access.next_entry::<K, V>()? {
            if map.insert(key, value).is_some() {
                return Err(A::Error::custom(format!("duplicate {} {key:?}", self.what)));
            }
        }
        Ok(map)
    }
}

/// The most a decode reserves on a document's word alone.
const MAX_PREALLOC_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
struct NodesVisitor;

impl<'de> Visitor<'de> for NodesVisitor {
    type Value = HashMap<NodeId, Node>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map of node ids to nodes")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        // The hint is the document's own claim, so it is capped, as serde caps its own
        // collections, to what a megabyte of entries holds.
        let cap = MAX_PREALLOC_BYTES / size_of::<(NodeId, Node)>();
        let mut nodes = HashMap::with_capacity(access.size_hint().unwrap_or_default().min(cap));
        while let Some((node_id, node)) = access.next_entry::<NodeId, Node>()? {
            if nodes.insert(node_id, node).is_some() {
                return Err(A::Error::custom(format!("duplicate node id {node_id:?}")));
            }
        }
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests;
