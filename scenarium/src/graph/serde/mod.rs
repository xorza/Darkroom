//! Field codecs for [`Graph`](crate::graph::Graph)'s tables and a set's compounds. A map built by
//! repeated `insert` keeps the last value for a repeated key, so a corrupt document would load as
//! a quietly different graph. Each codec decodes entry by entry and rejects the collision, since
//! validation could only assert after the evidence was gone.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use ::serde::de::Error as _;
use ::serde::de::{MapAccess, Visitor};
use ::serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::binding::Binding;
use crate::graph::compound::Compound;
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{GhostBinding, GhostSubscription};
use crate::graph::identity::{CompoundId, FuncId, GraphId, InputPort, NodeId};
use crate::graph::interface_change::WiringChange;
use crate::graph::node::Node;
use crate::graph::saved_decl::SavedDecl;
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

/// An interface change's graphs, refusing a graph given twice.
pub(super) fn deserialize_wiring_changes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<GraphId, WiringChange>, D::Error> {
    unique_pairs(deserializer, "wiring change of graph")
}

/// The node table, decoded as a map like the derive would, but refusing a repeated id: two
/// entries under one id are two different nodes, and keeping the last silently rebinds every
/// wire naming that id. Only here do both entries still exist.
pub(super) fn deserialize_nodes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<NodeId, Node>, D::Error> {
    deserializer.deserialize_map(UniqueMapVisitor {
        what: "node id",
        entry: PhantomData,
    })
}

/// The compound table, decoded as a map refusing a repeated id, as [`deserialize_nodes`] does
/// for nodes: two compounds under one id are two definitions every instance of either names. A
/// compound under the nil id is refused too: no set may hold one, and unlike a node, which a load
/// moves to the ghosts, it has no graph to be repaired in.
pub(super) fn deserialize_compounds<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<CompoundId, Compound>, D::Error> {
    let compounds = deserializer.deserialize_map(UniqueMapVisitor {
        what: "compound id",
        entry: PhantomData,
    })?;
    if compounds.contains_key(&CompoundId::nil()) {
        return Err(D::Error::custom("a compound under the nil id"));
    }
    Ok(compounds)
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

#[cfg(test)]
mod tests;
