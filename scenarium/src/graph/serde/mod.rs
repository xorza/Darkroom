//! Field codecs for [`Graph`](crate::graph::Graph)'s tables and a set's subgraphs. A map built by
//! repeated `insert` keeps the last value for a repeated key, so a corrupt document would load as
//! a quietly different graph. Each codec decodes entry by entry and rejects the collision, since
//! validation could only assert after the evidence was gone.

use std::collections::BTreeMap;
use std::fmt;

use ::serde::de::Error as _;
use ::serde::de::{MapAccess, Visitor};
use ::serde::{Deserialize, Deserializer, Serializer};
use hashbrown::HashMap;

use crate::graph::binding::Binding;
use crate::graph::identity::{InputPort, NodeId, SubgraphId};
use crate::graph::node::Node;
use crate::graph::stub::Stub;
use crate::graph::subgraph::Subgraph;

/// The binding table travels as a sequence of `(port, binding)` pairs.
pub(super) fn serialize_bindings<S: Serializer>(
    map: &BTreeMap<InputPort, Binding>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(map)
}

pub(super) fn deserialize_bindings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<InputPort, Binding>, D::Error> {
    let entries = Vec::<(InputPort, Binding)>::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for (port, binding) in entries {
        if map.insert(port, binding).is_some() {
            return Err(D::Error::custom(format!(
                "duplicate binding for input port {port:?}"
            )));
        }
    }
    Ok(map)
}

/// The stubs travel as the list of stubs, in node-id order.
pub(super) fn serialize_stubs<S: Serializer>(
    map: &BTreeMap<NodeId, Stub>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(map.values())
}

/// Refuses a list that repeats a node id: two stubs under one id are two nodes, and keeping the
/// last loses the other's settings.
pub(super) fn deserialize_stubs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<NodeId, Stub>, D::Error> {
    let stubs = Vec::<Stub>::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for stub in stubs {
        let node_id = stub.node_id();
        if map.insert(node_id, stub).is_some() {
            return Err(D::Error::custom(format!(
                "duplicate stub for node {node_id:?}"
            )));
        }
    }
    Ok(map)
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
    deserializer.deserialize_map(SubgraphsVisitor)
}

#[derive(Debug)]
struct SubgraphsVisitor;

impl<'de> Visitor<'de> for SubgraphsVisitor {
    type Value = BTreeMap<SubgraphId, Subgraph>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map of subgraph ids to subgraphs")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut subgraphs = BTreeMap::new();
        while let Some((id, subgraph)) = access.next_entry::<SubgraphId, Subgraph>()? {
            if subgraphs.insert(id, subgraph).is_some() {
                return Err(A::Error::custom(format!("duplicate subgraph id {id:?}")));
            }
        }
        Ok(subgraphs)
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
