use std::collections::BTreeMap;

use common::SerdeFormat;
use hashbrown::HashMap;
use ron::ser;
use serde::de::value::{self, MapAccessDeserializer};
use serde::de::{DeserializeSeed, MapAccess};
use serde::{Deserialize, Deserializer, Serialize};

use crate::graph::identity::{FuncId, InputPort, NodeId};
use crate::graph::node::{Node, NodeKind};
use crate::graph::serde::{deserialize_bindings, deserialize_nodes};
use crate::graph::{Binding, Graph};

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
    let port = InputPort::new(NodeId::unique(), 0);
    let bindings = RawBindings(vec![
        (port, Binding::Const(1i64.into())),
        (port, Binding::Const(2i64.into())),
    ]);

    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&bindings).unwrap();
        let error = format
            .deserialize::<CheckedBindings>(&bytes)
            .expect_err("a repeated input port does not decode")
            .to_string();

        if format != SerdeFormat::Bitcode || cfg!(debug_assertions) {
            assert!(
                error.contains("duplicate binding for input port"),
                "unexpected {format:?} error: {error}"
            );
        }
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
/// Written as raw RON because that is the only way the collision can be
/// *expressed*: every in-memory form of a node table has already collapsed
/// it. RON hands both entries to the visitor rather than deduping while
/// parsing, which is what leaves the check something to find.
#[test]
fn a_repeated_node_id_is_refused_rather_than_collapsed() {
    let node_id = NodeId::unique();
    let key = ser::to_string(&node_id).expect("a node id is a string");
    let first =
        ser::to_string(&Node::new(NodeKind::Func(FuncId::unique()))).expect("a node serializes");
    let second =
        ser::to_string(&Node::new(NodeKind::Func(FuncId::unique()))).expect("a node serializes");

    // The derived `HashMap` decode keeps the last entry and reports success;
    // pinning that here is what makes the refusal below a behaviour rather
    // than a coincidence of the fixture.
    let table = format!("{{{key}:{first},{key}:{second}}}");
    let kept: HashMap<NodeId, Node> =
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
