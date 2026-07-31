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
mod tests {
    use std::collections::BTreeMap;

    use hashbrown::HashMap;
    use serde::{Deserialize, Deserializer, Serialize};

    use crate::graph::identity::{FuncId, InputPort, NodeId};
    use crate::graph::node::{Node, NodeKind};
    use crate::graph::{Binding, Graph};
    use ::common::{SerdeFormat, deserialize, serialize};

    use super::{deserialize_bindings, deserialize_nodes};

    #[derive(Debug, Serialize)]
    #[serde(transparent)]
    struct RawBindings(Vec<(InputPort, Binding)>);

    #[derive(Debug)]
    struct CheckedBindings;

    impl<'de> Deserialize<'de> for CheckedBindings {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let _: BTreeMap<InputPort, Binding> = deserialize_bindings(deserializer)?;
            Ok(Self)
        }
    }

    #[test]
    fn duplicate_ports_fail_in_every_binding_format() {
        let port = InputPort::new(NodeId::unique(), 0);
        let bindings = RawBindings(vec![
            (port, Binding::Const(1i64.into())),
            (port, Binding::Const(2i64.into())),
        ]);

        for format in [SerdeFormat::Json, SerdeFormat::Bitcode] {
            let bytes = serialize(&bindings, format).unwrap();
            let error = deserialize::<CheckedBindings>(&bytes, format)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("duplicate binding for input port"),
                "unexpected {format:?} error: {error}"
            );
        }
    }

    #[derive(Debug)]
    struct CheckedNodes;

    impl<'de> Deserialize<'de> for CheckedNodes {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let _: HashMap<NodeId, Node> = deserialize_nodes(deserializer)?;
            Ok(Self)
        }
    }

    /// A document naming one id twice is refused, rather than loading as a
    /// graph one node smaller.
    ///
    /// Written as raw JSON because that is the only way the collision can be
    /// *expressed*: every in-memory form of a node table has already collapsed
    /// it. `serde_json` hands both entries to the visitor rather than deduping
    /// while parsing, which is what leaves the check something to find.
    #[test]
    fn a_repeated_node_id_is_refused_rather_than_collapsed() {
        let node_id = NodeId::unique();
        let key = serde_json::to_string(&node_id).expect("a node id is a json string");
        let first = serde_json::to_string(&Node::new(NodeKind::Func(FuncId::unique())))
            .expect("a node serializes");
        let second = serde_json::to_string(&Node::new(NodeKind::Func(FuncId::unique())))
            .expect("a node serializes");

        // The derived `HashMap` decode keeps the last entry and reports success;
        // pinning that here is what makes the refusal below a behaviour rather
        // than a coincidence of the fixture.
        let table = format!("{{{key}:{first},{key}:{second}}}");
        let kept: HashMap<NodeId, Node> =
            serde_json::from_str(&table).expect("the derived decode absorbs the collision");
        assert_eq!(kept.len(), 1, "the derived decode loses one of the two");

        let error = serde_json::from_str::<CheckedNodes>(&table)
            .expect_err("the checked decode refuses the collision");
        assert!(
            error.to_string().contains("duplicate node id"),
            "unexpected error: {error}"
        );

        // And through `Graph` itself, where the field attribute has to be wired
        // up for any of this to apply.
        let document = format!(r#"{{"nodes":{table},"bindings":[],"subscriptions":[]}}"#);
        let error = serde_json::from_str::<Graph>(&document)
            .expect_err("a graph holding a repeated node id does not load");
        assert!(
            error.to_string().contains("duplicate node id"),
            "unexpected error: {error}"
        );

        // One entry per id still loads, so the check refuses collisions rather
        // than maps.
        let document = format!(r#"{{"nodes":{{{key}:{first}}},"bindings":[],"subscriptions":[]}}"#);
        let graph: Graph = serde_json::from_str(&document).expect("a well-formed document loads");
        assert_eq!(graph.len(), 1);
    }
}
