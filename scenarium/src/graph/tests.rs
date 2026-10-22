use std::error::Error;

use ron::ser;
use ron::value::RawValue;

use crate::data::const_value::ConstValue;
use crate::data::data_type::{DataType, TypeId};
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::{DetachedNode, DetachedNodeParts};
use crate::graph::error::{
    BindingRejection, DetachedNodeError, GraphValidationError, SubscriptionRejection, Unresolved,
};
use crate::graph::func::Func;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::identity::{EventPort, FuncId};
use crate::graph::node::{CacheMode, Node, NodeKind};
use crate::graph::reconciled::Repair;
use crate::graph::subscription::Subscription;
use crate::graph::{Graph, InputPort, NodeId, OutputPort};
use crate::internals;
use crate::internals::test_graph::TestGraph;
use common::SerdeFormat;
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

    // A `Func` kind names an id and a signature, and resolving it takes a
    // library this constructor does not have. So it starts unnamed and uncached — and an
    // empty name means exactly that: a node whose func nothing has resolved.
    let unresolved = internals::unresolved_node(FuncId::unique());
    assert_eq!(unresolved.cache, CacheMode::None);
    assert!(unresolved.name.is_empty());
}

#[test]
fn const_only_input_rejects_bind_but_a_normal_input_accepts_it() {
    // One Int-in / Int-out func, so a wire between two of its instances is
    // otherwise valid — only the `const_only` flag decides whether validation
    // accepts it.
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
    use crate::data::data_type::{FsPathConfig, FsPathMode};
    use std::sync::Arc;

    let mut g = TestGraph::new();
    g.add("int_src", |n| n.pure().output(DataType::Int));
    g.add("str_sink", |n| n.sink().input(DataType::String));
    g.add("int_sink", |n| n.sink().input(DataType::Int));
    let wire = Binding::wire(g.id("int_src"), 0);
    let admit = |sink: &str| {
        g.graph
            .admit(&g.library, InputPort::new(g.id(sink), 0), &wire)
    };
    assert_eq!(admit("str_sink"), Err(BindingRejection::WireMismatch));
    assert_eq!(admit("int_sink"), Ok(()));

    let path_type = |mode| DataType::FsPath(Arc::new(FsPathConfig::new(mode)));
    let admits = |declared: DataType, value: ConstValue| {
        let mut g = TestGraph::new();
        g.add("sink", |n| n.sink().input(declared));
        let port = InputPort::new(g.id("sink"), 0);
        g.graph.admit(&g.library, port, &Binding::Const(value))
    };
    let cases = [
        (DataType::Int, ConstValue::String("x".into()), false),
        (DataType::Int, ConstValue::Float(2.5), true),
        (
            path_type(FsPathMode::ExistingFile),
            ConstValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]),
            false,
        ),
        (
            path_type(FsPathMode::ExistingFiles),
            ConstValue::FsPath("a.fit".into()),
            false,
        ),
        (
            path_type(FsPathMode::ExistingFiles),
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
            admits(declared.clone(), value.clone()),
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
    let from_port: Binding = OutputPort::new(nid, 1).into();
    assert_eq!(from_port, Binding::wire(nid, 1));
    assert_eq!(from_port, Binding::Wire(OutputPort::new(nid, 1)));

    let from_value: Binding = ConstValue::Int(7).into();
    assert_eq!(from_value, Binding::Const(ConstValue::Int(7)));
}

#[test]
fn input_bindings_are_sparse_and_none_removes_an_entry() {
    let mut g = TestGraph::sample();

    let first = InputPort::new(g.id("sum"), 0);
    let second = InputPort::new(g.id("sum"), 1);
    let absent = InputPort::new(g.id("sum"), 2);
    assert_eq!(
        g.graph.bindings.get(&first),
        Some(&Binding::wire(g.id("get_a"), 0))
    );
    assert_eq!(
        g.graph.bindings.get(&second),
        Some(&Binding::wire(g.id("get_b"), 0))
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

    assert!(!g.graph.is_subscribed(EventPort::new(emitter, 0), sub));
    g.subscribe("get_a", 0, "sum");
    assert!(g.graph.is_subscribed(EventPort::new(emitter, 0), sub));

    // Distinct event_idx is a distinct edge.
    assert!(!g.graph.is_subscribed(EventPort::new(emitter, 1), sub));

    // Re-subscribing is idempotent (BTreeSet dedups).
    g.subscribe("get_a", 0, "sum");
    assert_eq!(g.graph.subscriptions().count(), 1);

    // One event carries several subscribers, and a second event off the same
    // emitter is its own edge — so the three coexist rather than overwriting.
    let (sub2, other) = (g.id("mult"), g.id("Print"));
    g.subscribe("get_a", 0, "mult");
    g.subscribe("get_a", 1, "Print");
    assert!(g.graph.is_subscribed(EventPort::new(emitter, 0), sub));
    assert!(g.graph.is_subscribed(EventPort::new(emitter, 0), sub2));
    assert!(g.graph.is_subscribed(EventPort::new(emitter, 1), other));
    assert!(!g.graph.is_subscribed(EventPort::new(emitter, 1), sub));

    // `subscriptions` yields them in (event, subscriber) order, whatever
    // order they were authored in — the determinism a compile's subscriber
    // lists inherit.
    let mut expected = vec![
        Subscription::new(EventPort::new(emitter, 0), sub),
        Subscription::new(EventPort::new(emitter, 0), sub2),
        Subscription::new(EventPort::new(emitter, 1), other),
    ];
    expected.sort();
    assert_eq!(g.graph.subscriptions().collect::<Vec<_>>(), expected);

    // Dropping one edge leaves its siblings alone.
    g.unsubscribe("get_a", 0, "sum");
    assert!(!g.graph.is_subscribed(EventPort::new(emitter, 0), sub));
    assert!(g.graph.is_subscribed(EventPort::new(emitter, 0), sub2));
    assert!(g.graph.is_subscribed(EventPort::new(emitter, 1), other));

    g.unsubscribe("get_a", 0, "mult");
    g.unsubscribe("get_a", 1, "Print");
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
    g.subscribe("get_a", 0, "sum");
    // Its own two inputs, in port order; the binding that reads it is not one.
    assert_eq!(
        g.graph
            .input_bindings(sum)
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        [InputPort::new(sum, 0), InputPort::new(sum, 1)]
    );

    let before = g.graph.clone_verbatim();
    let edges_before = g.graph.wires().count();
    let detached = g.remove("sum");
    assert_eq!(g.graph.wires().count(), edges_before - 3);
    assert!(!g.graph.is_subscribed(EventPort::new(get_a, 0), sum));
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

    let foreign_port = InputPort::new(mult, 1);
    let foreign = BindingEntry {
        port: foreign_port,
        binding: Binding::Const(1i64.into()),
    };
    let first = detached.bindings()[0].clone();
    let elsewhere = Subscription::new(EventPort::new(get_a, 0), mult);
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
    let attached = g.graph.clone_verbatim();
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
    assert_eq!(
        g.graph.find(id).unwrap().kind,
        NodeKind::Func(func.func_ref())
    );
    assert_eq!(
        g.graph.bindings.get(&InputPort::new(id, 0)),
        Some(&Binding::Const(7i64.into()))
    );

    g.add("sum", |n| {
        n.pure()
            .input(DataType::Int)
            .optional(DataType::Int)
            .output(DataType::Int)
    });
    let sum = g.id("sum");

    assert!(!g.graph.bindings.contains_key(&InputPort::new(sum, 0)));
    assert!(!g.graph.bindings.contains_key(&InputPort::new(sum, 1)));
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

/// The release-path structural guard on an untrusted document. Decoding does
/// not validate — a host loads with `deserialize` followed by
/// [`Graph::validate`], which is the only place every structural invariant the
/// graph's own mutations *assert* gets **checked** instead.
#[test]
fn loading_rejects_a_corrupt_graph() {
    // A mirror of `Graph`'s wire shape. `RawValue` keeps every field's exact text,
    // so the duplicated entry below is the only thing authored. The mirror names
    // every field, so a new one on `Graph` fails its decode, which points here.
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct WireGraph {
        nodes: Box<RawValue>,
        bindings: Vec<Box<RawValue>>,
        subscriptions: Box<RawValue>,
    }

    let mut g = TestGraph::sample();
    g.graph.set_input_binding(
        InputPort::new(g.id("sum"), 0),
        Binding::wire(NodeId::unique(), 0),
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
    assert!(
        error
            .to_string()
            .contains("duplicate binding for input port"),
        "{error}"
    );
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
    let admit = |port_idx, binding: Binding| {
        g.graph
            .admit(&g.library, InputPort::new(dst, port_idx), &binding)
    };
    assert_eq!(
        admit(5, Binding::Const(ConstValue::Int(1))),
        Err(BindingRejection::UndeclaredInput)
    );
    assert_eq!(
        admit(0, Binding::wire(src, 7)),
        Err(BindingRejection::UndeclaredOutput)
    );
    assert_eq!(admit(0, Binding::Const(ConstValue::Null)), Ok(()));
    assert_eq!(
        admit(1, Binding::Const(ConstValue::Null)),
        Err(BindingRejection::ConstMismatch)
    );

    let subscribe = |event_idx, subscriber| {
        let subscription = Subscription::new(EventPort::new(src, event_idx), subscriber);
        g.graph.admit_subscription(&g.library, subscription)
    };
    assert_eq!(subscribe(0, dst), Ok(()));
    assert_eq!(
        subscribe(3, dst),
        Err(SubscriptionRejection::UndeclaredEvent)
    );
    assert_eq!(
        subscribe(0, src),
        Err(SubscriptionRejection::SelfSubscription)
    );
}

/// A named edit of a func's declaration.
type FuncEdit = (&'static str, fn(&mut FuncBuilder));

/// A two-input, one-output, one-event func to hold a signature against.
fn signed_func() -> FuncBuilder {
    use crate::graph::func::FuncEvent;
    use crate::graph::func::func_input::FuncInput;
    use crate::graph::func::func_output::FuncOutput;
    FuncBuilder {
        inputs: vec![
            FuncInput::required("a", DataType::Int),
            FuncInput::optional("b", DataType::String),
        ],
        outputs: vec![FuncOutput::new("out", DataType::Float)],
        events: vec![FuncEvent {
            name: "tick".into(),
            event_lambda: internals::stub_event(),
        }],
        ..internals::stub_func(FuncId::from_u128(7), "signed")
    }
}

/// A signature covers what a document's wiring is indexed by — the ports' names, types and
/// order, and the events' names — and nothing a binding does not land by.
#[test]
fn a_signature_covers_the_ports_and_nothing_else() {
    use crate::data::data_type::{FsPathConfig, FsPathMode};
    use std::sync::Arc;

    let signature = signed_func().build().func_ref();
    let edited = |edit: fn(&mut FuncBuilder)| {
        let mut func = signed_func();
        edit(&mut func);
        func.build().func_ref()
    };
    let moves: [FuncEdit; 7] = [
        ("an input renamed", |f| f.inputs[0].name = "x".into()),
        ("an input retyped", |f| {
            f.inputs[0].data_type = DataType::Float;
        }),
        ("the inputs reordered", |f| f.inputs.swap(0, 1)),
        ("an output dropped", |f| f.outputs.clear()),
        ("an output retyped", |f| {
            f.outputs[0].data_type = DataType::Int;
        }),
        ("an event renamed", |f| f.events[0].name = "tock".into()),
        ("a custom type swapped", |f| {
            f.inputs[1].data_type = DataType::Custom(TypeId::from_u128(1));
        }),
    ];
    for (what, edit) in moves {
        assert_ne!(edited(edit), signature, "{what}");
    }
    let keeps: [FuncEdit; 4] = [
        ("the func renamed", |f| f.name = "other".into()),
        ("an input made optional", |f| f.inputs[0].required = false),
        ("a description added", |f| {
            f.inputs[0].description = Some("explained".into());
        }),
        ("a default value set", |f| {
            f.inputs[0].default_value = Some(ConstValue::Int(3));
        }),
    ];
    for (what, edit) in keeps {
        assert_eq!(edited(edit), signature, "{what}");
    }
    // A path picker's mode decides what it offers, not where its binding lands.
    let path = |mode| {
        let mut func = signed_func();
        func.inputs[1].data_type = DataType::FsPath(Arc::new(FsPathConfig::new(mode)));
        func.build().func_ref()
    };
    assert_eq!(path(FsPathMode::Dir), path(FsPathMode::NewFile));
}

/// A node resolves to the func with the ports it was authored against; a func
/// whose ports moved since, and one the library lacks, are told apart.
#[test]
fn func_of_resolves_a_node_or_says_why_not() {
    let mut g = TestGraph::new();
    g.add("f", |n| n.pure().input(DataType::Int).output(DataType::Int));
    let id = g.id("f");
    let func_id = g.graph.find(id).unwrap().func_id().unwrap();
    let resolved = |g: &TestGraph| g.library.func_of(g.graph.find(id).unwrap()).map(Func::id);
    assert_eq!(resolved(&g), Ok(func_id));

    g.edit_func("f", |func| func.inputs[0].data_type = DataType::String);
    assert_eq!(resolved(&g), Err(Unresolved::ChangedPorts));

    let missing = internals::unresolved_node(FuncId::from_u128(99));
    assert_eq!(
        g.library.func_of(&missing).map(Func::id),
        Err(Unresolved::MissingFunc)
    );
}

/// A reconcile removes exactly what the library cannot hold, reports each
/// removal in a fixed order, detaches each node that does not resolve with the
/// wiring that touched it — an edge between two such nodes going to the one
/// detached first — and leaves a graph that validates and compiles. `Null`
/// stays on an optional input.
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
    g.add("changed", |n| {
        n.pure().input(DataType::Int).output(DataType::Int)
    });
    g.add("gone", |n| n.pure().output(DataType::Int));
    g.add("a", |n| n.pure().input(DataType::Int).output(DataType::Int));
    g.instance("b", "a");
    let (src, dst, changed, gone) = (g.id("src"), g.id("dst"), g.id("changed"), g.id("gone"));
    let nil_func = NodeId::from_u128(50);
    let (absent_consumer, absent_emitter) = (NodeId::from_u128(77), NodeId::from_u128(79));

    g.constant("dst", 0, ConstValue::Null);
    g.wire(("src", 7), ("dst", 1));
    g.wire("src", ("dst", 2));
    g.constant("dst", 3, 1i64);
    g.wire("src", ("dst", 4));
    g.constant("dst", 5, 1i64);
    g.wire("gone", "changed");
    g.wire("a", "b");
    g.wire("b", "a");
    g.subscribe("src", 0, "dst");
    g.subscribe("src", 0, "src");
    g.subscribe("src", 3, "dst");
    g.subscribe("src", 0, "gone");
    let absent_port = InputPort::new(absent_consumer, 0);
    g.graph
        .set_input_binding(absent_port, Binding::Const(ConstValue::Int(2)));
    let absent_event = EventPort::new(absent_emitter, 0);
    g.graph.subscribe(absent_event, dst);
    let src_func = g.library.by_name("src").unwrap();
    g.graph.nodes.insert(NodeId::nil(), Node::from(src_func));
    g.graph
        .insert(nil_func, internals::unresolved_node(FuncId::nil()));
    g.edit_func("changed", |func| {
        func.inputs[0].data_type = DataType::String;
    });
    let gone_func = g.graph.find(gone).unwrap().func_id().unwrap();
    g.library.remove(gone_func);

    let reconciled = g.graph.reconcile(&g.library);

    let rejected = |port_idx, binding, rejection| Repair::RejectedBinding {
        port: InputPort::new(dst, port_idx),
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
            rejected(1, Binding::wire(src, 7), BindingRejection::UndeclaredOutput),
            rejected(2, Binding::wire(src, 0), BindingRejection::WiredConstOnly),
            rejected(
                3,
                Binding::Const(ConstValue::Int(1)),
                BindingRejection::ConstMismatch
            ),
            rejected(4, Binding::wire(src, 0), BindingRejection::WireMismatch),
            rejected(
                5,
                Binding::Const(ConstValue::Int(1)),
                BindingRejection::UndeclaredInput
            ),
            Repair::RejectedSubscription {
                subscription: Subscription::new(EventPort::new(src, 0), src),
                rejection: SubscriptionRejection::SelfSubscription,
            },
            Repair::RejectedSubscription {
                subscription: Subscription::new(EventPort::new(src, 3), dst),
                rejection: SubscriptionRejection::UndeclaredEvent,
            },
            // The walk enters `a` first, reads `b`, and `b`'s wire from `a`
            // returns to the path.
            Repair::CycleWire {
                port: InputPort::new(g.id("b"), 0),
                producer: OutputPort::new(g.id("a"), 0),
            },
        ]
    );

    let detached: Vec<_> = reconciled
        .detached
        .iter()
        .map(|detachment| (detachment.record.node_id(), detachment.reason))
        .collect();
    assert_eq!(
        detached,
        [
            (changed, Unresolved::ChangedPorts),
            (gone, Unresolved::MissingFunc),
            (nil_func, Unresolved::MissingFunc),
        ]
    );
    let changed_record = &reconciled.detached[0].record;
    assert_eq!(
        changed_record.bindings(),
        [BindingEntry {
            port: InputPort::new(changed, 0),
            binding: Binding::wire(gone, 0),
        }],
        "detached first, so the edge to `gone` is `changed`'s"
    );
    let gone_record = &reconciled.detached[1].record;
    assert!(gone_record.bindings().is_empty());
    assert_eq!(
        gone_record.subscriptions(),
        [Subscription::new(EventPort::new(src, 0), gone)]
    );

    assert_eq!(
        g.graph.input_bindings(dst).collect::<Vec<_>>(),
        [(InputPort::new(dst, 0), &Binding::Const(ConstValue::Null))]
    );
    assert_eq!(
        g.graph.subscriptions().collect::<Vec<_>>(),
        [Subscription::new(EventPort::new(src, 0), dst)]
    );
    assert!(g.graph.validate_with(&g.library).is_ok());
    assert_eq!(g.compile().program.e_nodes.len(), 4);

    assert!(
        g.graph.reconcile(&g.library).is_empty(),
        "a reconciled graph needs nothing more"
    );
}

/// A node's signature survives the document round trip.
#[test]
fn a_signature_round_trips() -> TestResult {
    let node = Node::from(&signed_func().build());
    let encoded = SerdeFormat::Ron.serialize(&node)?;
    let decoded: Node = SerdeFormat::Ron.deserialize(&encoded)?;
    assert_eq!(decoded, node);
    Ok(())
}
