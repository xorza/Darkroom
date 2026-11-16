use std::collections::BTreeMap;

use common::{ErrorChain, SerdeFormat};
use ron::ser;
use serde::de::value::{self, MapAccessDeserializer};
use serde::de::{DeserializeSeed, MapAccess};
use serde::{Deserialize, Deserializer, Serialize};

use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::identity::PortId;
use crate::graph::identity::{FuncId, InputPort, NodeId};
use crate::graph::node::Node;
use crate::graph::serde::{deserialize_bindings, deserialize_nodes};
use crate::internals;

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

/// The refusal is asserted for every format. The reason is asserted only where it survives:
/// `bitcode::Error` is a zero-sized type outside debug builds and prints `bitcode error` there,
/// so a release run can prove the document does not decode but not which check refused it.
#[test]
fn duplicate_ports_fail_in_every_binding_format() {
    let port = InputPort::new(NodeId::unique(), PortId::new(1));
    let bindings = RawBindings(vec![
        (port, Binding::Const(1i64.into())),
        (port, Binding::Const(2i64.into())),
    ]);

    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&bindings).unwrap();
        let error = format
            .deserialize::<CheckedBindings>(&bytes)
            .expect_err("a repeated input port does not decode");
        let error = ErrorChain::new(&error).to_string();

        if format != SerdeFormat::Bitcode || cfg!(debug_assertions) {
            assert!(
                error.contains("duplicate binding for input port"),
                "unexpected {format:?} error: {error}"
            );
        }
    }
}

/// A graph saves the same bytes however its nodes came in, and walks them in id order: three
/// nodes inserted in one order and in its reverse serialize alike, and `iter` hands back their
/// ids sorted.
#[test]
fn a_graph_saves_and_walks_its_nodes_in_id_order() {
    let ids = [3, 1, 2].map(NodeId::from_u128);
    let build = |order: &[NodeId]| {
        let mut graph = Graph::default();
        for &node_id in order {
            graph.insert(node_id, internals::unresolved_node(FuncId::from_u128(9)));
        }
        graph
    };
    let forward = build(&ids);
    let mut reversed = ids;
    reversed.reverse();
    let backward = build(&reversed);
    for format in [SerdeFormat::Ron, SerdeFormat::Bitcode] {
        assert_eq!(
            format.serialize(&forward).unwrap(),
            format.serialize(&backward).unwrap(),
            "{format:?}"
        );
    }
    assert_eq!(
        forward.iter().map(|node| node.node_id).collect::<Vec<_>>(),
        [1, 2, 3].map(NodeId::from_u128)
    );
}

#[derive(Debug)]
struct CheckedNodes;

impl<'de> Deserialize<'de> for CheckedNodes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let _: BTreeMap<NodeId, Node> = deserialize_nodes(deserializer)?;
        Ok(Self)
    }
}

/// A document naming one id twice is refused, rather than loading as a
/// graph one node smaller.
///
/// Written as raw RON because every in-memory form of a node table has already collapsed the
/// collision; RON hands both entries to the visitor rather than deduping while parsing.
#[test]
fn a_repeated_node_id_is_refused_rather_than_collapsed() {
    let node_id = NodeId::unique();
    let key = ser::to_string(&node_id).expect("a node id is a string");
    let first =
        ser::to_string(&internals::unresolved_node(FuncId::unique())).expect("a node serializes");
    let second =
        ser::to_string(&internals::unresolved_node(FuncId::unique())).expect("a node serializes");

    // The derived `BTreeMap` decode keeps the last entry and reports success, so the refusal
    // below is a behaviour rather than a coincidence of the fixture.
    let table = format!("{{{key}:{first},{key}:{second}}}");
    let kept: BTreeMap<NodeId, Node> =
        ron::from_str(&table).expect("the derived decode absorbs the collision");
    assert_eq!(kept.len(), 1, "the derived decode loses one of the two");

    let error = ron::from_str::<CheckedNodes>(&table)
        .expect_err("the checked decode refuses the collision");
    assert!(
        error.to_string().contains("duplicate node id"),
        "unexpected error: {error}"
    );

    // And through `Graph` itself, where the field attribute has to be wired
    // up for any of this to apply.
    let document = format!("(nodes:{table},bindings:[],subscriptions:[])");
    let error = ron::from_str::<Graph>(&document)
        .expect_err("a graph holding a repeated node id does not load");
    assert!(
        error.to_string().contains("duplicate node id"),
        "unexpected error: {error}"
    );

    // One entry per id still loads, so the check refuses collisions rather
    // than maps.
    let document = format!("(nodes:{{{key}:{first}}},bindings:[],subscriptions:[])");
    let graph: Graph = ron::from_str(&document).expect("a well-formed document loads");
    assert_eq!(graph.len(), 1);
}

/// A map that claims more entries than any document could hold and then
/// yields none — the hint alone, with nothing behind it.
#[derive(Debug)]
struct OverclaimingMap;

impl<'de> MapAccess<'de> for OverclaimingMap {
    type Error = value::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        _seed: K,
    ) -> Result<Option<K::Value>, value::Error> {
        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        _seed: V,
    ) -> Result<V::Value, value::Error> {
        unreachable!("no key was yielded")
    }

    fn size_hint(&self) -> Option<usize> {
        Some(usize::MAX)
    }
}

/// The node table's length hint comes from the document, so a corrupt one
/// decodes to what the entries hold rather than reserving what it claims —
/// `usize::MAX` entries would abort on capacity overflow before the first
/// entry was read.
#[test]
fn a_node_table_overclaiming_its_length_reserves_only_what_decodes() {
    let nodes = deserialize_nodes(MapAccessDeserializer::new(OverclaimingMap))
        .expect("an empty table decodes");
    assert!(nodes.is_empty());
}
