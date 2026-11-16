use std::error::Error;

use ron::ser;
use ron::value::RawValue;

use crate::data::const_value::ConstValue;
use crate::data::data_type::file_kind::FileKindId;
use crate::data::data_type::{DataType, FileMode};
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::{DetachedNode, DetachedNodeParts};
use crate::graph::error::{
    BindingRejection, DetachReason, DetachedNodeError, GraphValidationError, SubscriptionRejection,
    Unresolved,
};
use crate::graph::func::Func;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::identity::{EventPort, FuncId, PortId};
use crate::graph::node::{CacheMode, Node, NodeKind};
use crate::graph::ports::{InputDecl, numbered_port};
use crate::graph::reconciled::Repair;
use crate::graph::stub::Stub;
use crate::graph::subscription::Subscription;
use crate::graph::{Graph, InputPort, NodeId, OutputPort};
use crate::internals;
use crate::internals::test_graph::TestGraph;
use common::{ErrorChain, SerdeFormat};
use std::panic;
use std::panic::AssertUnwindSafe;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn validate_passes_for_valid_graph() {
    assert!(TestGraph::sample().graph.validate().is_ok());
}

#[test]
fn cache_mode_bits_and_from_bits_round_trip() {
    // The two storage bits, hand-tabulated per mode, plus `from_bits` as their inverse.
    let table = [
        (CacheMode::None, false, false),
        (CacheMode::Ram, true, false),
        (CacheMode::Disk, false, true),
        (CacheMode::Both, true, true),
    ];
    for (mode, ram, disk) in table {
        assert_eq!(mode.caches_in_ram(), ram, "{mode:?} RAM bit");
        assert_eq!(mode.persists_to_disk(), disk, "{mode:?} disk bit");
        assert_eq!(
            CacheMode::from_bits(ram, disk),
            mode,
            "from_bits({ram},{disk})"
        );
    }
}

#[test]
fn cache_mode_round_trips() {
    assert_eq!(CacheMode::default(), CacheMode::None);

    for mode in [
        CacheMode::None,
        CacheMode::Ram,
        CacheMode::Disk,
        CacheMode::Both,
    ] {
        let mut g = TestGraph::new();
        g.add("src", |n| n.pure().output(DataType::Int));
        g.cache("src", mode);

        for format in SerdeFormat::ALL {
            let bytes = format.serialize(&g.graph).unwrap();
            let back: Graph = format.deserialize(&bytes).unwrap();
            assert_eq!(
                back.find_by_name("src").unwrap().cache,
                mode,
                "{mode:?} via {format:?}"
            );
        }
    }
}

#[test]
fn a_new_node_takes_what_its_declaration_says() {
    use crate::graph::node::special::SpecialNode;

    // A fresh func node inherits its func's `default_cache_mode` — the out-of-box
    // `None`, or whatever the func's builder raised it to.
    let plain = internals::stub_func(FuncId::unique(), "plain").build();
    assert_eq!(
        Node::from(&plain).cache,
        CacheMode::None,
        "default func → no caching"
    );

    let hot = internals::stub_func(FuncId::unique(), "hot")
        .default_cache_mode(CacheMode::Both)
        .build();
    assert_eq!(
        Node::from(&hot).cache,
        CacheMode::Both,
        "func node copies the func's default_cache_mode"
    );
    // `add_func_node` routes through the same `From<&Func>`, so the graph-level
    // constructor propagates it too.
    let mut graph = Graph::default();
    let id = graph.add_func_node(&hot);
    assert_eq!(graph.find(id).unwrap().cache, CacheMode::Both);

    assert_eq!(
        Node::from(&hot).name,
        "hot",
        "and the func's name, so a placed node is labelled without a second lookup"
    );

    // A special node's declaration is hardcoded, so `From<SpecialNode>` reaches
    // it and takes both halves — no caller has to name the node afterwards.
    let special = SpecialNode::RunSinks;
    let node = Node::from(special);
    assert_eq!(node.name, special.func().name());
    assert!(
        !node.name.is_empty(),
        "a hardcoded declaration always names it"
    );
    assert_eq!(node.cache, special.func().default_cache_mode());

    // Resolving a `Func` kind takes a library this constructor does not have, so it starts
    // unnamed and uncached: an empty name means a node whose func nothing has resolved.
    let unresolved = internals::unresolved_node(FuncId::unique());
    assert_eq!(unresolved.cache, CacheMode::None);
    assert!(unresolved.name.is_empty());
}

#[test]
fn const_only_input_rejects_bind_but_a_normal_input_accepts_it() {
    // One Int-in / Int-out func, so only the `const_only` flag decides whether validation
    // accepts a wire between two of its instances.
    let validate = |const_only: bool| -> Result<(), GraphValidationError> {
        let mut g = TestGraph::new();
        g.add("f", |n| {
            let n = n.pure().input(DataType::Int);
            let n = if const_only { n.const_only() } else { n };
            n.output(DataType::Int)
        });
        g.instance("consumer", "f");
        g.wire("f", "consumer");
        g.graph.validate_with(&g.library)
    };

    assert!(
        validate(false).is_ok(),
        "a normal input accepts a wired binding"
    );
    let err = validate(true).expect_err("a const-only input must reject a wired binding");
    assert!(
        err.to_string().contains("const-only"),
        "unexpected error: {err}"
    );
}

/// The type half of admission: an `Int` output feeds an `Int` input and not a
/// `String` one; a `String` literal never fits an `Int` input, a numeric one
/// does (scalar coercion), and the two path shapes fit only their own picker
/// mode.
#[test]
fn admit_holds_wires_and_consts_to_the_input_type() {
    let mut g = TestGraph::new();
    g.add("int_src", |n| n.pure().output(DataType::Int));
    g.add("str_sink", |n| n.sink().input(DataType::String));
    g.add("int_sink", |n| n.sink().input(DataType::Int));
    let wire = Binding::wire(g.id("int_src"), numbered_port(1));
    let admit = |sink: &str| {
        g.graph.admit(
            &g.library,
            InputPort::new(g.id(sink), numbered_port(1)),
            &wire,
        )
    };
    assert_eq!(admit("str_sink"), Err(BindingRejection::WireMismatch));
    assert_eq!(admit("int_sink"), Ok(()));

    let path_type = |mode| DataType::File {
        kind: FileKindId::from_u128(1),
        mode,
    };
    let admits = |declared: DataType, value: ConstValue| {
        let mut g = TestGraph::new();
        g.add("sink", |n| n.sink().input(declared));
        let port = InputPort::new(g.id("sink"), numbered_port(1));
        g.graph.admit(&g.library, port, &Binding::Const(value))
    };
    let cases = [
        (DataType::Int, ConstValue::String("x".into()), false),
        (DataType::Int, ConstValue::Float(2.5), true),
        (
            path_type(FileMode::Open),
            ConstValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]),
            false,
        ),
        (
            path_type(FileMode::OpenMany),
            ConstValue::FsPath("a.fit".into()),
            false,
        ),
        (
            path_type(FileMode::OpenMany),
            ConstValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]),
            true,
        ),
    ];
    for (declared, value, admitted) in cases {
        let expected = if admitted {
            Ok(())
        } else {
            Err(BindingRejection::ConstMismatch)
        };
        assert_eq!(
            admits(declared, value.clone()),
            expected,
            "const {value:?} on {declared:?}"
        );
    }
}

#[test]
fn removing_a_node_drops_it_and_every_edge_touching_it() {
    let mut g = TestGraph::sample();
    let sum = g.id("sum");

    g.remove("sum");

    assert!(g.graph.find_by_name("sum").is_none());
    assert_eq!(g.graph.len(), 4);

    // No surviving edge references the removed node (as consumer or producer).
    for (dst, src) in g.graph.wires() {
        assert_ne!(dst.node_id, sum);
        assert_ne!(src.node_id, sum);
    }
}

/// The rule the editor applies before it lets a wire land: a back-edge closes
/// a loop, a forward or sideways one does not, and a node wired to itself
/// always does. Direct *and* transitive, since the walk is what tells the two
/// apart.
#[test]
fn produces_cycle_detects_direct_and_transitive_loops() {
    // A node with one input and one output can chain: a → b → c, with d left
    // unconnected.
    let mut g = TestGraph::new();
    g.add("a", |n| n.input(DataType::Any).output(DataType::Int));
    g.instance("b", "a");
    g.instance("c", "a");
    g.instance("d", "a");
    g.wire("a", "b");
    g.wire("b", "c");

    let closes = |from: &str, to: &str| g.graph.produces_cycle(g.id(from), g.id(to));
    assert!(closes("b", "a"), "b → a closes a → b");
    assert!(closes("c", "a"), "c → a closes a → b → c transitively");
    assert!(closes("a", "a"), "a node wired to itself");

    // Forward and sideways edges are fine: a second a → c path is a DAG
    // diamond, and an unconnected node is reachable from nothing.
    assert!(!closes("a", "c"), "a → c is a second forward path");
    assert!(!closes("c", "d"), "d reads from nothing");
    assert!(!closes("a", "d"), "d reads from nothing");
}

/// The walk reads a node's inputs as one contiguous range of the binding map,
/// so a producer feeding a *high* port has to be found as surely as one on
/// port 0 — including across a const and an unbound port, neither of which is
/// an edge and neither of which may end the scan early.
#[test]
fn produces_cycle_reaches_a_producer_past_a_const_and_an_unbound_port() {
    let mut g = TestGraph::new();
    g.add("a", |n| n.input(DataType::Any).output(DataType::Int));
    g.instance("b", "a");
    g.add("wide", |n| {
        n.input(DataType::Any)
            .input(DataType::Any)
            .input(DataType::Any)
            .output(DataType::Int)
    });
    g.wire("a", "b");
    // The only wire into `wide` sits on port 2, behind a literal on port 0 and
    // nothing at all on port 1.
    g.constant("wide", 0, 7i64);
    g.wire("b", ("wide", 2));

    let closes = |from: &str, to: &str| g.graph.produces_cycle(g.id(from), g.id(to));
    assert!(
        closes("wide", "a"),
        "a → b → wide[2] means wide → a closes the loop"
    );
    assert!(
        closes("wide", "b"),
        "and the one-hop b → wide[2] back-edge too"
    );
    // The same wiring in the other direction is a plain forward edge, and the
    // const on port 0 is not an edge anything can be reached through.
    assert!(!closes("a", "wide"), "a reads from nothing");
    assert!(!closes("b", "wide"), "b reads only from a");
}

#[test]
fn binding_conversions() {
    let nid = NodeId::unique();
    let from_port: Binding = OutputPort::new(nid, numbered_port(2)).into();
    assert_eq!(from_port, Binding::wire(nid, numbered_port(2)));
    assert_eq!(
        from_port,
        Binding::Wire(OutputPort::new(nid, numbered_port(2)))
    );

    let from_value: Binding = ConstValue::Int(7).into();
    assert_eq!(from_value, Binding::Const(ConstValue::Int(7)));
}

#[test]
fn input_bindings_are_sparse_and_none_removes_an_entry() {
    let mut g = TestGraph::sample();

    let first = InputPort::new(g.id("sum"), numbered_port(1));
    let second = InputPort::new(g.id("sum"), numbered_port(2));
    let absent = InputPort::new(g.id("sum"), numbered_port(3));
    assert_eq!(
        g.graph.bindings.get(&first),
        Some(&Binding::wire(g.id("get_a"), numbered_port(1)))
    );
    assert_eq!(
        g.graph.bindings.get(&second),
        Some(&Binding::wire(g.id("get_b"), numbered_port(1)))
    );
    assert!(!g.graph.bindings.contains_key(&absent));

    let binding_count = g.graph.bindings.len();
    g.unbind("sum", 0);
    assert!(!g.graph.bindings.contains_key(&first));
    assert_eq!(g.graph.bindings.len(), binding_count - 1);
}

#[test]
fn subscribe_unsubscribe_is_subscribed() {
    let mut g = TestGraph::sample();
    let (emitter, sub) = (g.id("get_a"), g.id("sum"));

    assert!(
        !g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(1)), sub)
    );
    g.graph
        .subscribe(EventPort::new(emitter, numbered_port(1)), sub);
    assert!(
        g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(1)), sub)
    );

    // Distinct event_idx is a distinct edge.
    assert!(
        !g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(2)), sub)
    );

    // Re-subscribing is idempotent (BTreeSet dedups).
    g.graph
        .subscribe(EventPort::new(emitter, numbered_port(1)), sub);
    assert_eq!(g.graph.subscriptions().count(), 1);

    // One event carries several subscribers, and a second event off the same
    // emitter is its own edge — so the three coexist rather than overwriting.
    let (sub2, other) = (g.id("mult"), g.id("Print"));
    g.graph
        .subscribe(EventPort::new(emitter, numbered_port(1)), sub2);
    g.graph
        .subscribe(EventPort::new(emitter, numbered_port(2)), other);
    assert!(
        g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(1)), sub)
    );
    assert!(
        g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(1)), sub2)
    );
    assert!(
        g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(2)), other)
    );
    assert!(
        !g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(2)), sub)
    );

    // `subscriptions` yields them in (event, subscriber) order, whatever
    // order they were authored in — the determinism a compile's subscriber
    // lists inherit.
    let mut expected = vec![
        Subscription::new(EventPort::new(emitter, numbered_port(1)), sub),
        Subscription::new(EventPort::new(emitter, numbered_port(1)), sub2),
        Subscription::new(EventPort::new(emitter, numbered_port(2)), other),
    ];
    expected.sort();
    assert_eq!(g.graph.subscriptions().collect::<Vec<_>>(), expected);

    // Dropping one edge leaves its siblings alone.
    g.graph
        .unsubscribe(EventPort::new(emitter, numbered_port(1)), sub);
    assert!(
        !g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(1)), sub)
    );
    assert!(
        g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(1)), sub2)
    );
    assert!(
        g.graph
            .is_subscribed(EventPort::new(emitter, numbered_port(2)), other)
    );

    g.graph
        .unsubscribe(EventPort::new(emitter, numbered_port(1)), sub2);
    g.graph
        .unsubscribe(EventPort::new(emitter, numbered_port(2)), other);
    assert_eq!(g.graph.subscriptions().count(), 0);
}

/// A detached node carries exactly the wiring that touched it, and attaching it
/// restores the graph. `DetachedNode::new` puts a record from outside in the
/// graph's order and refuses one that could not have come out of a graph; a
/// record that could has to meet a graph it fits, or the attach panics with the
/// graph untouched. A nil id is simply absent from every lookup.
#[test]
fn a_detached_node_restores_its_wiring_and_malformed_records_are_refused() {
    let mut g = TestGraph::sample();
    assert!(g.graph.find(NodeId::nil()).is_none());
    assert!(g.library.by_id(FuncId::nil()).is_none());
    let sum = g.id("sum");
    let get_a = g.id("get_a");
    let mult = g.id("mult");
    // A subscription that touches `sum`, so both kinds of wiring travel.
    g.graph
        .subscribe(EventPort::new(get_a, numbered_port(1)), sum);
    // Its own two inputs, in port-id order; the binding that reads it is not one.
    assert_eq!(
        g.graph
            .input_bindings(sum)
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        [
            InputPort::new(sum, numbered_port(1)),
            InputPort::new(sum, numbered_port(2))
        ]
    );

    let before = g.graph.clone();
    let edges_before = g.graph.wires().count();
    let detached = g.remove("sum");
    assert_eq!(g.graph.wires().count(), edges_before - 3);
    assert!(
        !g.graph
            .is_subscribed(EventPort::new(get_a, numbered_port(1)), sum)
    );
    assert_eq!(detached.bindings().len(), 3);
    assert_eq!(detached.subscriptions().len(), 1);

    let rebuild = |bindings: Vec<BindingEntry>, subscriptions: Vec<Subscription>| {
        DetachedNode::new(DetachedNodeParts {
            node_id: sum,
            node: detached.node().clone(),
            bindings,
            subscriptions,
        })
    };
    let mut reversed = detached.bindings().to_vec();
    reversed.reverse();
    assert_eq!(
        rebuild(reversed, detached.subscriptions().to_vec()),
        Ok(detached.clone()),
        "a record from outside is put in the graph's order"
    );

    let foreign_port = InputPort::new(mult, numbered_port(2));
    let foreign = BindingEntry {
        port: foreign_port,
        binding: Binding::Const(1i64.into()),
    };
    let first = detached.bindings()[0].clone();
    let elsewhere = Subscription::new(EventPort::new(get_a, numbered_port(1)), mult);
    let own = detached.subscriptions()[0];
    let refusals = [
        (
            DetachedNode::new(DetachedNodeParts {
                node_id: NodeId::nil(),
                node: detached.node().clone(),
                bindings: Vec::new(),
                subscriptions: Vec::new(),
            }),
            DetachedNodeError::NilNodeId,
        ),
        (
            rebuild(vec![foreign], Vec::new()),
            DetachedNodeError::ForeignBinding { port: foreign_port },
        ),
        (
            rebuild(vec![first.clone(), first.clone()], Vec::new()),
            DetachedNodeError::DuplicateBinding { port: first.port },
        ),
        (
            rebuild(Vec::new(), vec![elsewhere]),
            DetachedNodeError::ForeignSubscription {
                subscription: elsewhere,
            },
        ),
        (
            rebuild(Vec::new(), vec![own, own]),
            DetachedNodeError::DuplicateSubscription { subscription: own },
        ),
    ];
    for (result, error) in refusals {
        assert_eq!(result, Err(error));
    }

    // Serialized, a record round-trips; deserializing goes through the same
    // refusals, so a record edited into a nil id fails to decode.
    let bytes = SerdeFormat::Bitcode.serialize(&detached).unwrap();
    let decoded: DetachedNode = SerdeFormat::Bitcode.deserialize(&bytes).unwrap();
    assert_eq!(decoded, detached);
    let text = String::from_utf8(SerdeFormat::Ron.serialize(&detached).unwrap()).unwrap();
    let nil = text.replace(&sum.to_string(), &NodeId::nil().to_string());
    assert_ne!(nil, text, "the id is spelled in the text");
    assert!(
        SerdeFormat::Ron
            .deserialize::<DetachedNode>(nil.as_bytes())
            .is_err()
    );

    g.graph.attach_node(detached.clone());
    assert_eq!(g.graph, before);
    let attached = g.graph.clone();
    let twice = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph.attach_node(detached);
    }));
    assert!(
        twice.is_err(),
        "a node already in the graph is not attached again"
    );
    assert_eq!(g.graph, attached, "a refused attach leaves the graph alone");
}

/// Placing a node seeds a const binding for every input its declaration gave a
/// default, and nothing at all for the ones it did not — required and optional
/// alike, so an absent binding means "no default" rather than "not optional".
#[test]
fn add_func_node_seeds_only_the_inputs_with_defaults() {
    let mut g = TestGraph::new();
    g.add("withdefault", |n| {
        n.pure()
            .defaulted(DataType::Int, 7i64)
            .output(DataType::Int)
    });
    let id = g.id("withdefault");

    let func = g.library.by_name("withdefault").unwrap();
    assert_eq!(g.graph.find(id).unwrap().kind, NodeKind::Func(func.id()));
    assert_eq!(
        g.graph.bindings.get(&InputPort::new(id, numbered_port(1))),
        Some(&Binding::Const(7i64.into()))
    );

    g.add("sum", |n| {
        n.pure()
            .input(DataType::Int)
            .optional(DataType::Int)
            .output(DataType::Int)
    });
    let sum = g.id("sum");

    assert!(
        !g.graph
            .bindings
            .contains_key(&InputPort::new(sum, numbered_port(1)))
    );
    assert!(
        !g.graph
            .bindings
            .contains_key(&InputPort::new(sum, numbered_port(2)))
    );
}

#[test]
fn serialization_round_trips_a_graph_through_every_format() -> TestResult {
    let graph = TestGraph::sample().graph;
    for format in SerdeFormat::ALL {
        let serialized = format.serialize(&graph)?;
        let deserialized: Graph = format.deserialize(&serialized)?;
        assert_eq!(graph, deserialized, "{format:?} round-trips a graph whole");
    }
    Ok(())
}

/// The release-path structural guard on an untrusted document. Decoding does not validate: a
/// host loads with `deserialize` then [`Graph::validate`], the only place every structural
/// invariant the graph's mutations *assert* is **checked**.
#[test]
fn loading_rejects_a_corrupt_graph() {
    // A mirror of `Graph`'s wire shape. `RawValue` keeps every field's exact text, so the
    // duplicated entry below is the only thing authored. Naming every field makes a new one on
    // `Graph` fail its decode, which points here.
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct WireGraph {
        nodes: Box<RawValue>,
        bindings: Vec<Box<RawValue>>,
        subscriptions: Box<RawValue>,
        stubs: Vec<Box<RawValue>>,
    }

    let mut g = TestGraph::sample();
    g.graph.set_input_binding(
        InputPort::new(g.id("sum"), numbered_port(1)),
        Binding::wire(NodeId::unique(), numbered_port(1)),
    );
    let bytes = SerdeFormat::Bitcode.serialize(&g.graph).unwrap();
    let decoded: Graph = SerdeFormat::Bitcode
        .deserialize(&bytes)
        .expect("a structurally broken graph still decodes; validation is what rejects it");
    let error = decoded
        .validate()
        .expect_err("a binding naming a node the document doesn't hold is rejected");
    assert!(matches!(
        error,
        GraphValidationError::BindingMissingProducer { .. }
    ));
    assert!(
        error.to_string().contains("binds to missing node"),
        "the message names what broke: {error}"
    );

    let mut nil_key = Graph::default();
    nil_key
        .nodes
        .insert(NodeId::nil(), internals::unresolved_node(FuncId::unique()));
    let bytes = SerdeFormat::Bitcode.serialize(&nil_key).unwrap();
    let decoded: Graph = SerdeFormat::Bitcode.deserialize(&bytes).unwrap();
    assert!(matches!(
        decoded.validate(),
        Err(GraphValidationError::NilNodeId)
    ));

    // Bindings decode from a sequence into a map, so a repeated input port is
    // caught during decode rather than by validation after it.
    let encoded = ser::to_string(&TestGraph::sample().graph).unwrap();
    let mut wire: WireGraph = ron::from_str(&encoded).expect("the wire shape still matches");
    wire.bindings.push(wire.bindings[0].clone());
    let bytes = ser::to_string(&wire).unwrap().into_bytes();
    let error = SerdeFormat::Ron
        .deserialize::<Graph>(&bytes)
        .expect_err("a duplicate input port cannot decode into the binding map");
    let message = ErrorChain::new(&error).to_string();
    assert!(
        message.contains("duplicate binding for input port"),
        "{message}"
    );

    // Stubs decode from a sequence into a map too, so a repeated node id is caught in the decode;
    // a stub under a node's id decodes, and validation names it.
    let mut stubbed = TestGraph::sample();
    let get_a = stubbed.id("get_a");
    let record = stubbed.graph.detach_node(get_a);
    stubbed.graph.insert_stub(Stub {
        record: record.clone(),
        reason: DetachReason::Unresolved(Unresolved::MissingFunc),
    });
    let encoded = ser::to_string(&stubbed.graph).unwrap();
    let mut wire: WireGraph = ron::from_str(&encoded).expect("the wire shape still matches");
    wire.stubs.push(wire.stubs[0].clone());
    let bytes = ser::to_string(&wire).unwrap().into_bytes();
    let error = SerdeFormat::Ron
        .deserialize::<Graph>(&bytes)
        .expect_err("a repeated stub id cannot decode into the stub map");
    let message = ErrorChain::new(&error).to_string();
    assert!(message.contains("duplicate stub for node"), "{message}");
    let mut shadowed = stubbed.graph.clone();
    shadowed.nodes.insert(get_a, record.node().clone());
    assert!(matches!(
        shadowed.validate(),
        Err(GraphValidationError::StubHoldsNode { node_id }) if node_id == get_a
    ));
}

/// A graph keeps each node it cannot hold as a stub beside its nodes: the ids of the two are
/// disjoint, so neither an insert nor a stub takes an id the other holds; a stub round-trips with
/// the graph; and the constants of the graph are its bindings' and its stubs', for reading and
/// for writing.
#[test]
fn a_graph_keeps_its_stubs_beside_its_nodes() {
    let mut g = TestGraph::sample();
    g.constant("sum", 1, 4i64);
    let (get_b, sum) = (g.id("get_b"), g.id("sum"));
    let record = g.graph.detach_node(sum);
    let reason = DetachReason::Unresolved(Unresolved::MissingFunc);
    g.graph.insert_stub(Stub {
        record: record.clone(),
        reason,
    });
    let graph = g.graph.clone();
    assert!(graph.find(sum).is_none());
    assert_eq!(graph.stub(sum).map(|stub| stub.reason), Some(reason));
    assert!(graph.holds(sum));
    assert!(graph.holds(get_b));
    assert!(!graph.holds(NodeId::unique()));
    assert_eq!(graph.len(), 4, "a stub is no node");

    let node_over_stub = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph.insert(sum, record.node().clone());
    }));
    assert!(node_over_stub.is_err(), "a node takes no stub's id");
    let shadow = g.graph.snapshot_node(get_b).unwrap();
    let stub_over_node = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph.insert_stub(Stub {
            record: shadow,
            reason,
        });
    }));
    assert!(stub_over_node.is_err(), "a stub takes no node's id");

    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&graph).unwrap();
        assert_eq!(
            format.deserialize::<Graph>(&bytes).unwrap(),
            graph,
            "{format:?}"
        );
    }

    let into_sum = InputPort::new(sum, numbered_port(2));
    assert_eq!(
        graph.constants().collect::<Vec<_>>(),
        [(into_sum, &ConstValue::Int(4))],
        "the sample's only constant sits in the stub's record"
    );
    let mut written = graph;
    for value in written.constants_mut() {
        *value = ConstValue::Int(9);
    }
    assert_eq!(
        written.stub(sum).unwrap().record.bindings()[1],
        BindingEntry {
            port: into_sum,
            binding: Binding::Const(ConstValue::Int(9)),
        }
    );
    written.validate().unwrap();
}

/// Admission refuses wiring past what a node declares — an input, a
/// producer's output, an event — and a node listening to itself; `Null` fits
/// an optional input and no required one.
#[test]
fn admit_refuses_undeclared_ports_events_and_a_self_subscription() {
    let mut g = TestGraph::new();
    g.add("src", |n| {
        n.output(DataType::Int)
            .event("tick", internals::stub_event())
    });
    g.add("dst", |n| {
        n.sink().optional(DataType::Int).input(DataType::Int)
    });
    let (src, dst) = (g.id("src"), g.id("dst"));
    let admit = |number, binding: Binding| {
        g.graph.admit(
            &g.library,
            InputPort::new(dst, numbered_port(number)),
            &binding,
        )
    };
    assert_eq!(
        admit(6, Binding::Const(ConstValue::Int(1))),
        Err(BindingRejection::UndeclaredInput)
    );
    assert_eq!(
        admit(1, Binding::wire(src, numbered_port(8))),
        Err(BindingRejection::UndeclaredOutput)
    );
    assert_eq!(admit(1, Binding::Const(ConstValue::Null)), Ok(()));
    assert_eq!(
        admit(2, Binding::Const(ConstValue::Null)),
        Err(BindingRejection::ConstMismatch)
    );

    let subscribe = |number, subscriber| {
        let subscription =
            Subscription::new(EventPort::new(src, numbered_port(number)), subscriber);
        g.graph.admit_subscription(&g.library, subscription)
    };
    assert_eq!(subscribe(1, dst), Ok(()));
    assert_eq!(
        subscribe(4, dst),
        Err(SubscriptionRejection::UndeclaredEvent)
    );
    assert_eq!(
        subscribe(1, src),
        Err(SubscriptionRejection::SelfSubscription)
    );
}

/// A saved node's wiring names ports by id, so a func's ports may change under it: a load keeps
/// every binding whose port is still declared and still admits it, and refuses the rest, each
/// reported. A port gained leaves the node's wiring as it was, the new input unbound; a rename and
/// a reorder change nothing; a port that leaves takes its own binding alone; a retype refuses
/// what the new type does not take.
#[test]
fn a_func_whose_ports_change_keeps_each_binding_its_ports_still_admit() {
    let fixture = || {
        let mut g = TestGraph::new();
        g.add("src", |n| n.pure().output(DataType::Int));
        g.add("dst", |n| {
            n.sink()
                .input(DataType::Int)
                .optional(DataType::Int)
                .optional(DataType::Int)
        });
        g.wire("src", ("dst", 0));
        g.constant("dst", 1, 2i64);
        g.constant("dst", 2, 3i64);
        g
    };
    let reconciled = |edit: fn(&mut FuncBuilder)| {
        let mut g = fixture();
        g.edit_func("dst", edit);
        let report = g.graph.reconcile(&g.library);
        assert!(g.graph.find(g.id("dst")).is_some(), "the node resolves");
        let bindings: Vec<(PortId, Binding)> = g
            .graph
            .input_bindings(g.id("dst"))
            .map(|(port, binding)| (port.port, binding.clone()))
            .collect();
        (bindings, report.repairs)
    };
    let src = fixture().id("src");
    let all = vec![
        (numbered_port(1), Binding::wire(src, numbered_port(1))),
        (numbered_port(2), Binding::Const(ConstValue::Int(2))),
        (numbered_port(3), Binding::Const(ConstValue::Int(3))),
    ];

    let gained: fn(&mut FuncBuilder) = |f| {
        f.ports.inputs.push(InputDecl {
            id: numbered_port(4),
            ..InputDecl::required("new", DataType::Int)
        });
    };
    let renamed_and_reordered: fn(&mut FuncBuilder) = |f| {
        f.ports.inputs[0].name = "renamed".into();
        f.ports.inputs.swap(1, 2);
    };
    for edit in [gained, renamed_and_reordered] {
        assert_eq!(reconciled(edit), (all.clone(), vec![]));
    }

    let (bindings, repairs) = reconciled(|f| {
        f.ports.inputs.remove(1);
    });
    assert_eq!(bindings, [all[0].clone(), all[2].clone()]);
    let dst = fixture().id("dst");
    assert_eq!(
        repairs,
        [Repair::RejectedBinding {
            port: InputPort::new(dst, numbered_port(2)),
            binding: Binding::Const(ConstValue::Int(2)),
            rejection: BindingRejection::UndeclaredInput,
        }]
    );

    let (bindings, repairs) = reconciled(|f| {
        f.ports.inputs[0].data_type = DataType::String;
        f.ports.inputs[2].data_type = DataType::String;
    });
    assert_eq!(bindings, [all[1].clone()]);
    assert_eq!(
        repairs,
        [
            Repair::RejectedBinding {
                port: InputPort::new(dst, numbered_port(1)),
                binding: Binding::wire(src, numbered_port(1)),
                rejection: BindingRejection::WireMismatch,
            },
            Repair::RejectedBinding {
                port: InputPort::new(dst, numbered_port(3)),
                binding: Binding::Const(ConstValue::Int(3)),
                rejection: BindingRejection::ConstMismatch,
            },
        ]
    );
}

/// A node resolves to the func under its id, whatever its ports are now; one the library lacks
/// does not.
#[test]
fn func_of_resolves_a_node_or_says_why_not() {
    let mut g = TestGraph::new();
    g.add("f", |n| n.pure().input(DataType::Int).output(DataType::Int));
    let id = g.id("f");
    let func_id = g.graph.find(id).unwrap().func_id().unwrap();
    let resolved = |g: &TestGraph| g.library.func_of(g.graph.find(id).unwrap()).map(Func::id);
    assert_eq!(resolved(&g), Ok(func_id));

    g.edit_func("f", |func| {
        func.ports.inputs[0].data_type = DataType::String;
    });
    assert_eq!(resolved(&g), Ok(func_id));

    let missing = internals::unresolved_node(FuncId::from_u128(99));
    assert_eq!(
        g.library.func_of(&missing).map(Func::id),
        Err(Unresolved::MissingFunc)
    );
}

/// A reconcile removes exactly what the library cannot hold, reports each removal in a fixed
/// order, detaches each unresolved node with the wiring that touched it (an edge between two
/// such nodes going to the one detached first), and leaves a graph that validates and compiles.
/// `Null` stays on an optional input.
#[test]
fn reconcile_removes_exactly_what_the_library_cannot_hold() {
    let mut g = TestGraph::new();
    g.add("src", |n| {
        n.output(DataType::Int)
            .event("tick", internals::stub_event())
    });
    g.add("dst", |n| {
        n.sink()
            .optional(DataType::Int)
            .input(DataType::Int)
            .optional(DataType::Int)
            .const_only()
            .input(DataType::String)
            .input(DataType::String)
    });
    g.add("gone", |n| n.pure().output(DataType::Int));
    g.add("a", |n| n.pure().input(DataType::Int).output(DataType::Int));
    g.instance("b", "a");
    let (src, dst, gone) = (g.id("src"), g.id("dst"), g.id("gone"));
    let nil_func = NodeId::from_u128(50);
    let (absent_consumer, absent_emitter) = (NodeId::from_u128(77), NodeId::from_u128(79));

    let dst_port = |number| InputPort::new(dst, numbered_port(number));
    g.constant("dst", 0, ConstValue::Null);
    g.graph
        .set_input_binding(dst_port(2), Binding::wire(src, numbered_port(8)));
    g.wire("src", ("dst", 2));
    g.constant("dst", 3, 1i64);
    g.wire("src", ("dst", 4));
    g.graph
        .set_input_binding(dst_port(6), Binding::Const(ConstValue::Int(1)));
    g.wire("a", "b");
    g.wire("b", "a");
    g.subscribe("src", 0, "dst");
    g.subscribe("src", 0, "src");
    g.graph
        .subscribe(EventPort::new(src, numbered_port(4)), dst);
    g.subscribe("src", 0, "gone");
    let absent_port = InputPort::new(absent_consumer, numbered_port(1));
    g.graph
        .set_input_binding(absent_port, Binding::Const(ConstValue::Int(2)));
    let absent_event = EventPort::new(absent_emitter, numbered_port(1));
    g.graph.subscribe(absent_event, dst);
    let src_func = g.library.by_name("src").unwrap();
    g.graph.nodes.insert(NodeId::nil(), Node::from(src_func));
    g.graph
        .insert(nil_func, internals::unresolved_node(FuncId::nil()));
    let gone_func = g.graph.find(gone).unwrap().func_id().unwrap();
    g.library.remove(gone_func);

    let reconciled = g.graph.reconcile(&g.library);

    let rejected = |number, binding, rejection| Repair::RejectedBinding {
        port: dst_port(number),
        binding,
        rejection,
    };
    assert_eq!(
        reconciled.repairs,
        [
            Repair::NilNode,
            Repair::DanglingBinding {
                port: absent_port,
                binding: Binding::Const(ConstValue::Int(2)),
            },
            Repair::DanglingSubscription {
                subscription: Subscription::new(absent_event, dst),
            },
            Repair::Stubbed {
                node_id: gone,
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            },
            Repair::Stubbed {
                node_id: nil_func,
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            },
            rejected(
                2,
                Binding::wire(src, numbered_port(8)),
                BindingRejection::UndeclaredOutput
            ),
            rejected(
                3,
                Binding::wire(src, numbered_port(1)),
                BindingRejection::WiredConstOnly
            ),
            rejected(
                4,
                Binding::Const(ConstValue::Int(1)),
                BindingRejection::ConstMismatch
            ),
            rejected(
                5,
                Binding::wire(src, numbered_port(1)),
                BindingRejection::WireMismatch
            ),
            rejected(
                6,
                Binding::Const(ConstValue::Int(1)),
                BindingRejection::UndeclaredInput
            ),
            Repair::RejectedSubscription {
                subscription: Subscription::new(EventPort::new(src, numbered_port(1)), src),
                rejection: SubscriptionRejection::SelfSubscription,
            },
            Repair::RejectedSubscription {
                subscription: Subscription::new(EventPort::new(src, numbered_port(4)), dst),
                rejection: SubscriptionRejection::UndeclaredEvent,
            },
            // The walk enters `a` first, reads `b`, and `b`'s wire from `a`
            // returns to the path.
            Repair::CycleWire {
                port: InputPort::new(g.id("b"), numbered_port(1)),
                producer: OutputPort::new(g.id("a"), numbered_port(1)),
            },
        ]
    );

    assert_eq!(
        g.graph.stubs().map(Stub::node_id).collect::<Vec<_>>(),
        [gone, nil_func],
        "each node it detached is a stub of the graph"
    );
    let gone_record = &g.graph.stub(gone).unwrap().record;
    assert!(gone_record.bindings().is_empty());
    assert_eq!(
        gone_record.subscriptions(),
        [Subscription::new(
            EventPort::new(src, numbered_port(1)),
            gone
        )]
    );

    assert_eq!(
        g.graph.input_bindings(dst).collect::<Vec<_>>(),
        [(
            InputPort::new(dst, numbered_port(1)),
            &Binding::Const(ConstValue::Null)
        )]
    );
    assert_eq!(
        g.graph.subscriptions().collect::<Vec<_>>(),
        [Subscription::new(
            EventPort::new(src, numbered_port(1)),
            dst
        )]
    );
    assert!(g.graph.validate_with(&g.library).is_ok());
    assert_eq!(g.compile().program.e_nodes.len(), 4);

    assert!(
        g.graph.reconcile(&g.library).is_empty(),
        "a reconciled graph needs nothing more"
    );
}
