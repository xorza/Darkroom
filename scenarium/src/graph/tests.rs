use std::error::Error;

use ron::ser;
use ron::value::RawValue;

use crate::execution::compile::compiled_graph::ExecutionBinding;
use crate::graph::Graph;
use crate::graph::error::{DetachedNodeError, GraphValidationError};
use crate::graph::func::Func;
use crate::graph::identity::FuncId;
use crate::graph::node::{CacheMode, Node, NodeKind};
use crate::graph::{Binding, BindingEntry, InputPort, NodeId, OutputPort, Subscription};
use crate::internals;
use crate::internals::graph::{NodeSpec, TestGraph};
use crate::{ConstValue, DataType, DetachedNode};
use common::{SerdeFormat, deserialize, serialize};
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
            let bytes = serialize(&g.graph, format).unwrap();
            let back: Graph = deserialize(&bytes, format).unwrap();
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
    let plain = internals::stub_func(FuncId::unique(), "plain");
    assert_eq!(
        Node::from(&plain).cache,
        CacheMode::None,
        "default func → no caching"
    );

    let hot = internals::stub_func(FuncId::unique(), "hot").default_cache_mode(CacheMode::Both);
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

    // A special node's declaration is hardcoded, so `Node::new` reaches it and
    // takes both halves — no caller has to name the node afterwards.
    let special = SpecialNode::RunSinks;
    let node = Node::new(NodeKind::Special(special));
    assert_eq!(node.name, special.func().name);
    assert!(
        !node.name.is_empty(),
        "a hardcoded declaration always names it"
    );
    assert_eq!(node.cache, special.func().default_cache_mode);

    // A `Func` kind names only an id, and resolving one takes a library this
    // constructor does not have. So it starts unnamed and uncached — and an
    // empty name means exactly that: a node whose func nothing has resolved.
    let unresolved = Node::new(NodeKind::Func(FuncId::unique()));
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
        g.wire("f", 0, "consumer", 0);
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

#[test]
fn type_mismatches_degrade_at_lowering_not_at_validation() {
    use crate::{FsPathConfig, FsPathMode};
    use std::sync::Arc;

    // Int and String never coerce (numerics coerce among themselves, but a
    // string is a distinct kind), so this pair exercises a real mismatch.
    // Validation always accepts; the compiled program's lowered input shows
    // whether the binding survived the type gate or degraded to unbound.
    let mut g = TestGraph::new();
    g.add("int_src", |n| n.pure().output(DataType::Int));
    g.add("str_sink", |n| n.sink().input(DataType::String));
    g.add("int_sink", |n| n.sink().input(DataType::Int));
    g.wire("int_src", 0, "str_sink", 0);
    g.wire("int_src", 0, "int_sink", 0);

    assert!(g.graph.validate_with(&g.library).is_ok());
    let compiled = g.compile();
    assert!(
        matches!(compiled.binding("str_sink", 0), ExecutionBinding::None),
        "Int into a String input lowers as unbound"
    );
    assert!(
        matches!(compiled.binding("int_sink", 0), ExecutionBinding::Bind(_)),
        "Int into an Int input binds"
    );

    // Constants: a String literal can't satisfy an Int input, a numeric one
    // can (scalar coercion), and the two FsPath shapes only satisfy their
    // matching picker mode.
    let path_type = |mode| DataType::FsPath(Arc::new(FsPathConfig::new(mode)));
    let satisfies = |declared: DataType, value: ConstValue| {
        let mut g = TestGraph::new();
        g.add("sink", |n| n.sink().input(declared));
        g.constant("sink", 0, value);
        assert!(g.graph.validate_with(&g.library).is_ok());
        matches!(g.compile().binding("sink", 0), ExecutionBinding::Const(_))
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
    for (declared, value, expected) in cases {
        assert_eq!(
            satisfies(declared.clone(), value.clone()),
            expected,
            "const {value:?} on {declared:?}"
        );
    }
}

#[test]
fn resolve_output_type_follows_passthrough_chain() {
    // Int-out producer → pass1 → pass2. Both passthroughs declare an `Any`
    // (wildcard) output, but the resolved type must be the producer's `Int`.
    let mut g = TestGraph::new();
    g.add("src", |n| n.pure().output(DataType::Int));
    g.add("pass1", NodeSpec::passthrough);
    g.instance("pass2", "pass1");
    g.wire("src", 0, "pass1", 0);
    g.wire("pass1", 0, "pass2", 0);

    // The producer reports its own declared type.
    assert_eq!(g.output_type("src", 0), DataType::Int);
    // Each passthrough mirrors what flows through, transitively.
    assert_eq!(g.output_type("pass1", 0), DataType::Int);
    assert_eq!(g.output_type("pass2", 0), DataType::Int);

    // An unbound value input leaves the passthrough polymorphic (`Any`),
    // so its output accepts any consumer again.
    g.unbind("pass1", 0);
    assert_eq!(g.output_type("pass1", 0), DataType::Any);
    // The taint flows downstream: pass2 now reads pass1's `Any`.
    assert_eq!(g.output_type("pass2", 0), DataType::Any);

    // A scalar const carries its type, so the output resolves to it (and
    // propagates downstream) — a const isn't "no type".
    g.constant("pass1", 0, ConstValue::Bool(true));
    assert_eq!(g.output_type("pass1", 0), DataType::Bool);
    assert_eq!(
        g.output_type("pass2", 0),
        DataType::Bool,
        "the const's type propagates through the second passthrough too"
    );

    // A const whose type can't be reconstructed from the value alone — an
    // enum literal on an `Any` (wildcard) input — stays polymorphic rather
    // than panicking. (The passthrough's value input is `Any`-declared.)
    g.constant("pass1", 0, ConstValue::Enum("X".into()));
    assert_eq!(g.output_type("pass1", 0), DataType::Any);
}

#[test]
fn resolve_output_type_uses_declared_type_for_typed_const_input() {
    use crate::{FsPathConfig, FsPathMode, TypeId};
    use std::sync::Arc;

    // A reroute node with *typed* inputs, each mirrored by a wildcard output.
    let fs_ty = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)));
    let enum_ty = DataType::Enum(TypeId::from_u128(0x5e));
    let mut g = TestGraph::new();
    let (path_ty, mode_ty) = (fs_ty.clone(), enum_ty.clone());
    g.add("reroute", move |n| {
        n.input(path_ty).input(mode_ty).wildcard(0).wildcard(1)
    });

    // A const FsPath / Enum on a typed input resolves to that input's
    // *declared* type — which carries the full `FsPathConfig` / `Enum` id the
    // bare `ConstValue` lacks.
    g.constant("reroute", 0, ConstValue::FsPath("/tmp/x".into()));
    g.constant("reroute", 1, ConstValue::Enum("A".into()));
    assert_eq!(g.output_type("reroute", 0), fs_ty);
    assert_eq!(g.output_type("reroute", 1), enum_ty);
}

#[test]
fn type_mismatched_wiring_lowers_as_unbound_through_wildcard_chains() {
    let mut g = TestGraph::new();
    g.add("float_src", |n| n.pure().output(DataType::Float));
    g.add("str_src", |n| n.pure().output(DataType::String));
    g.add("pass1", NodeSpec::passthrough);
    g.instance("pass2", "pass1");
    g.add("sink", |n| n.sink().input(DataType::Float));
    g.wire("float_src", 0, "pass1", 0);
    g.wire("pass1", 0, "pass2", 0);
    g.wire("pass2", 0, "sink", 0);

    // The valid Float chain binds the sink to its passthrough producer
    // (passthroughs are real func nodes — only boundaries short-circuit).
    assert_eq!(
        g.compile().producer("sink", 0),
        Some("pass2"),
        "a well-typed chain lowers as bound"
    );

    // Rewire pass1's value input to the String producer: pass1.out and
    // pass2.out both retype to String, so the *two-hops-down* sink edge is
    // the one now incompatible — it lowers as unbound while the authored
    // wire survives in the document.
    g.wire("str_src", 0, "pass1", 0);
    assert_eq!(g.compile().producer("sink", 0), None);
    assert_eq!(
        g.graph.bindings.get(&InputPort::new(g.id("sink"), 0)),
        Some(&Binding::bind(g.id("pass2"), 0)),
        "the mismatched wire stays authored"
    );
}

#[test]
fn removing_a_node_drops_it_and_every_edge_touching_it() {
    let mut g = TestGraph::sample();
    let sum = g.id("sum");

    g.remove("sum");

    assert!(g.graph.find_by_name("sum").is_none());
    assert_eq!(g.graph.len(), 4);

    // No surviving edge references the removed node (as consumer or producer).
    for (dst, src) in g.graph.edges() {
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
    // A passthrough is both consumer and producer, so it can chain:
    // a → b → c, with d left unconnected.
    let mut g = TestGraph::new();
    g.add("a", NodeSpec::passthrough);
    g.instance("b", "a");
    g.instance("c", "a");
    g.instance("d", "a");
    g.wire("a", 0, "b", 0);
    g.wire("b", 0, "c", 0);

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
    g.add("a", NodeSpec::passthrough);
    g.instance("b", "a");
    g.add("wide", |n| {
        n.input(DataType::Any)
            .input(DataType::Any)
            .input(DataType::Any)
            .wildcard(0)
    });
    g.wire("a", 0, "b", 0);
    // The only wire into `wide` sits on port 2, behind a literal on port 0 and
    // nothing at all on port 1.
    g.constant("wide", 0, 7i64);
    g.wire("b", 0, "wide", 2);

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
    assert_eq!(from_port, Binding::bind(nid, 1));
    assert_eq!(from_port, Binding::Bind(OutputPort::new(nid, 1)));

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
        Some(&Binding::bind(g.id("get_a"), 0))
    );
    assert_eq!(
        g.graph.bindings.get(&second),
        Some(&Binding::bind(g.id("get_b"), 0))
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

    assert!(!g.graph.is_subscribed(emitter, 0, sub));
    g.subscribe("get_a", 0, "sum");
    assert!(g.graph.is_subscribed(emitter, 0, sub));

    // Distinct event_idx is a distinct edge.
    assert!(!g.graph.is_subscribed(emitter, 1, sub));

    // Re-subscribing is idempotent (BTreeSet dedups).
    g.subscribe("get_a", 0, "sum");
    assert_eq!(g.graph.subscriptions().count(), 1);

    // One event carries several subscribers, and a second event off the same
    // emitter is its own edge — so the three coexist rather than overwriting.
    let (sub2, other) = (g.id("mult"), g.id("Print"));
    g.subscribe("get_a", 0, "mult");
    g.subscribe("get_a", 1, "Print");
    assert!(g.graph.is_subscribed(emitter, 0, sub));
    assert!(g.graph.is_subscribed(emitter, 0, sub2));
    assert!(g.graph.is_subscribed(emitter, 1, other));
    assert!(!g.graph.is_subscribed(emitter, 1, sub));

    // `subscriptions` yields them in (emitter, event_idx, subscriber) order,
    // whatever order they were authored in — the determinism a compile's
    // subscriber lists inherit.
    let mut expected = vec![
        Subscription {
            emitter,
            event_idx: 0,
            subscriber: sub,
        },
        Subscription {
            emitter,
            event_idx: 0,
            subscriber: sub2,
        },
        Subscription {
            emitter,
            event_idx: 1,
            subscriber: other,
        },
    ];
    expected.sort();
    assert_eq!(g.graph.subscriptions().collect::<Vec<_>>(), expected);

    // Dropping one edge leaves its siblings alone.
    g.unsubscribe("get_a", 0, "sum");
    assert!(!g.graph.is_subscribed(emitter, 0, sub));
    assert!(g.graph.is_subscribed(emitter, 0, sub2));
    assert!(g.graph.is_subscribed(emitter, 1, other));

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
    let edges_before = g.graph.edges().count();
    let detached = g.remove("sum");
    assert_eq!(g.graph.edges().count(), edges_before - 3);
    assert!(!g.graph.is_subscribed(get_a, 0, sum));
    assert_eq!(detached.bindings().len(), 3);
    assert_eq!(detached.subscriptions().len(), 1);

    let rebuild = |bindings: Vec<BindingEntry>, subscriptions: Vec<Subscription>| {
        DetachedNode::new(sum, detached.node().clone(), bindings, subscriptions)
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
    let elsewhere = Subscription {
        emitter: get_a,
        event_idx: 0,
        subscriber: mult,
    };
    let own = detached.subscriptions()[0];
    let refusals = [
        (
            DetachedNode::new(
                NodeId::nil(),
                detached.node().clone(),
                Vec::new(),
                Vec::new(),
            ),
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
    let bytes = serialize(&detached, SerdeFormat::Bitcode).unwrap();
    let decoded: DetachedNode = deserialize(&bytes, SerdeFormat::Bitcode).unwrap();
    assert_eq!(decoded, detached);
    let text = String::from_utf8(serialize(&detached, SerdeFormat::Ron).unwrap()).unwrap();
    let nil = text.replace(&sum.to_string(), &NodeId::nil().to_string());
    assert_ne!(nil, text, "the id is spelled in the text");
    assert!(deserialize::<DetachedNode>(nil.as_bytes(), SerdeFormat::Ron).is_err());

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

    assert_eq!(
        g.graph.find(id).unwrap().kind,
        NodeKind::Func(g.library.by_name("withdefault").unwrap().id)
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
        let serialized = serialize(&graph, format)?;
        let deserialized: Graph = deserialize(&serialized, format)?;
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
    #[derive(serde::Serialize, serde::Deserialize)]
    struct WireGraph {
        nodes: Box<RawValue>,
        bindings: Vec<Box<RawValue>>,
        subscriptions: Box<RawValue>,
    }

    let mut g = TestGraph::sample();
    g.graph.set_input_binding(
        InputPort::new(g.id("sum"), 0),
        Binding::bind(NodeId::unique(), 0),
    );
    let bytes = serialize(&g.graph, SerdeFormat::Bitcode).unwrap();
    let decoded: Graph = deserialize(&bytes, SerdeFormat::Bitcode)
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
        .insert(NodeId::nil(), Node::new(NodeKind::Func(FuncId::unique())));
    let bytes = serialize(&nil_key, SerdeFormat::Bitcode).unwrap();
    let decoded: Graph = deserialize(&bytes, SerdeFormat::Bitcode).unwrap();
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
    let error = deserialize::<Graph>(&bytes, SerdeFormat::Ron)
        .expect_err("a duplicate input port cannot decode into the binding map");
    assert!(
        error
            .to_string()
            .contains("duplicate binding for input port"),
        "{error}"
    );
}

/// Wiring the current library can't resolve is tolerated, never a validation
/// error: it degrades to unbound at lowering time and revives if the library
/// grows the ports back. The counterpart to
/// [`type_mismatches_degrade_at_lowering_not_at_validation`].
#[test]
fn validate_tolerates_library_range_drift() {
    let mut g = TestGraph::new();
    g.add("one_out", |n| n.pure().output(DataType::Int));
    assert!(g.graph.validate_with(&g.library).is_ok());

    // Input 5, output 7, and event 3 are all past what `one_out` declares.
    let one_out = g.id("one_out");
    g.graph
        .set_input_binding(InputPort::new(one_out, 5), Binding::bind(one_out, 7));
    g.graph.subscribe(one_out, 3, one_out);
    assert!(g.graph.validate_with(&g.library).is_ok());

    // `Null` consts ("explicitly unset") are tolerated on both sides:
    // meaningful on an optional input, degrading to a missing input on a
    // required one at lowering (see `const_satisfies`).
    g.add("nullable", |n| {
        n.pure()
            .optional(DataType::Int)
            .input(DataType::Int)
            .output(DataType::Int)
    });
    g.constant("nullable", 0, ConstValue::Null);
    g.constant("nullable", 1, ConstValue::Null);
    assert!(g.graph.validate_with(&g.library).is_ok());
}

/// `Some(ports)` is an authoritative arity a caller may range-check against;
/// `None` means "unknowable here" and must *not* read as an empty port list —
/// the drift guards do `is_some_and(|p| idx >= p.len())`, so the two decide
/// opposite ways.
#[test]
fn a_node_resolves_its_declaration_or_reads_unknown() {
    let mut g = TestGraph::new();
    g.add("sum", |n| {
        n.pure()
            .input(DataType::Int)
            .optional(DataType::Int)
            .output(DataType::Int)
    });

    let declared = g.library.by_name("sum").unwrap().clone();
    let node = g.graph.find(g.id("sum")).unwrap();
    let ports = node.func(&g.library).unwrap();
    assert_eq!(ports.name, "sum");
    assert_eq!(ports.inputs.len(), declared.inputs.len());
    assert_eq!(ports.id, declared.id);

    // Library drift is unknown, not empty — otherwise every port on a node
    // whose func went missing would read as out of range.
    let missing_func = Node::new(NodeKind::Func(FuncId::unique()));
    assert!(missing_func.func(&g.library).is_none());
}

#[test]
fn input_type_resolves_declared_types_and_rejects_out_of_range() {
    let mut g = TestGraph::new();
    g.add("dst", |n| {
        n.pure().input(DataType::Float).output(DataType::Float)
    });
    let dst = g.id("dst");

    assert_eq!(
        g.graph.input_type(&g.library, InputPort::new(dst, 0)),
        Some(DataType::Float)
    );
    assert_eq!(g.graph.input_type(&g.library, InputPort::new(dst, 9)), None);
}

/// A named edit of a func's declaration.
type FuncEdit = (&'static str, fn(&mut Func));

/// A two-input, one-output, one-event func to hold a signature against.
fn signed_func() -> Func {
    use crate::graph::func::{FuncEvent, FuncInput, FuncOutput};
    Func {
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
    use crate::graph::func::OutputType;
    use crate::graph::func::signature::FuncSignature;
    use crate::{FsPathConfig, FsPathMode};
    use std::sync::Arc;

    let signature = FuncSignature::of(&signed_func());
    let edited = |edit: fn(&mut Func)| {
        let mut func = signed_func();
        edit(&mut func);
        FuncSignature::of(&func)
    };
    let moves: [FuncEdit; 7] = [
        ("an input renamed", |f| f.inputs[0].name = "x".into()),
        ("an input retyped", |f| {
            f.inputs[0].data_type = DataType::Float;
        }),
        ("the inputs reordered", |f| f.inputs.swap(0, 1)),
        ("an output dropped", |f| f.outputs.clear()),
        ("an output made a wildcard", |f| {
            f.outputs[0].ty = OutputType::Wildcard { mirrors: 0 };
        }),
        ("an event renamed", |f| f.events[0].name = "tock".into()),
        ("a custom type swapped", |f| {
            f.inputs[1].data_type = DataType::Custom(crate::TypeId::from_u128(1));
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
        FuncSignature::of(&func)
    };
    assert_eq!(path(FsPathMode::Directory), path(FsPathMode::NewFile));
}

/// A node holds the signature of the func it was made from. Once the library's func moves its
/// ports, validation and reconciling both refuse the node by name; a node that recorded none
/// adopts the library's; and a func the library lacks is left to render as a stub.
#[test]
fn a_node_authored_against_other_ports_is_refused_by_name() {
    use crate::graph::func::signature::FuncSignature;

    let mut g = TestGraph::new();
    g.add("f", |n| n.pure().input(DataType::Int).output(DataType::Int));
    let id = g.id("f");
    let func_id = match g.graph.find(id).unwrap().kind {
        NodeKind::Func(func_id) => func_id,
        NodeKind::Special(_) => unreachable!("the fixture adds a func node"),
    };
    let current = FuncSignature::of(g.library.by_id(func_id).unwrap());
    assert_eq!(g.graph.find(id).unwrap().signature, Some(current));
    assert!(g.graph.reconcile_signatures(&g.library).is_ok());

    g.edit_func("f", |func| func.inputs[0].data_type = DataType::String);
    for error in [
        g.graph.validate_with(&g.library).unwrap_err(),
        g.graph.reconcile_signatures(&g.library).unwrap_err(),
    ] {
        assert!(
            matches!(&error, GraphValidationError::SignatureMismatch { node_id, func_id: f, .. }
                if *node_id == id && *f == func_id),
            "{error}"
        );
    }

    g.graph.find_mut(id).unwrap().signature = None;
    g.graph.reconcile_signatures(&g.library).unwrap();
    assert_eq!(
        g.graph.find(id).unwrap().signature,
        Some(FuncSignature::of(g.library.by_id(func_id).unwrap()))
    );

    let stub = g
        .graph
        .add(Node::new(NodeKind::Func(FuncId::from_u128(99))));
    assert!(g.graph.reconcile_signatures(&g.library).is_ok());
    assert_eq!(g.graph.find(stub).unwrap().signature, None);
}

/// A node's signature survives the document round trip, and a document saved before
/// signatures were recorded loads with none — for reconciling to adopt.
#[test]
fn a_signature_round_trips_and_an_older_document_has_none() -> TestResult {
    let node = Node::from(&signed_func());
    let encoded = serialize(&node, SerdeFormat::Ron)?;
    let decoded: Node = deserialize(&encoded, SerdeFormat::Ron)?;
    assert_eq!(decoded, node);

    let older: String = String::from_utf8(encoded)?
        .lines()
        .filter(|line| !line.contains("signature:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!older.contains("signature"), "the older form has no field");
    let decoded: Node = deserialize(older.as_bytes(), SerdeFormat::Ron)?;
    assert_eq!(decoded.signature, None);
    Ok(())
}
