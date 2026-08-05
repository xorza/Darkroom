//! Field codecs for [`Graph`](crate::graph::Graph)'s tables — each one there
//! because the container the field decodes *into* silently absorbs something a
//! document must not contain.
//!
//! A `BTreeMap`/`HashMap` built by repeated `insert` keeps the last value for a
//! repeated key and drops the rest, so a corrupt document loads as a smaller,
//! quietly different graph rather than failing. Both codecs below decode entry
//! by entry and reject the collision instead, which is what makes "keys are
//! unique" a property of every `Graph` that exists rather than one validation
//! could only ever assert after the evidence was gone.

use std::collections::BTreeMap;
use std::fmt;

use ::serde::de::Error as _;
use ::serde::de::{MapAccess, Visitor};
use ::serde::{Deserialize, Deserializer, Serialize, Serializer};
use hashbrown::HashMap;

use crate::graph::Binding;
use crate::graph::identity::{InputPort, NodeId};
use crate::graph::node::Node;

/// Struct keys cannot be map keys in string-keyed formats such as JSON and TOML,
/// so the binding table travels as a sequence of pairs.
pub(super) fn serialize_bindings<S: Serializer>(
    map: &BTreeMap<InputPort, Binding>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    map.iter().collect::<Vec<_>>().serialize(serializer)
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

/// The node table, decoded as a map like the derive would — but refusing a
/// repeated id.
///
/// This is the one collision that loses *authored data*: two entries under one
/// id are two different nodes, and keeping the last silently rebinds every wire
/// naming that id onto a node with other ports. Nothing downstream can tell
/// that happened, so it has to be caught here — the only point where both
/// entries still exist.
pub(super) fn deserialize_nodes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<NodeId, Node>, D::Error> {
    deserializer.deserialize_map(NodesVisitor)
}

#[derive(Debug)]
struct NodesVisitor;

impl<'de> Visitor<'de> for NodesVisitor {
    type Value = HashMap<NodeId, Node>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map of node ids to nodes")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut nodes = HashMap::with_capacity(access.size_hint().unwrap_or_default());
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
