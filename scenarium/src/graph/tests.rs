use std::error::Error;

use ron::ser;
use ron::value::RawValue;

use crate::data::const_value::ConstValue;
use crate::data::data_type::file_kind::FileKindId;
use crate::data::data_type::{DataType, FileMode};
use crate::graph::binding::Binding;
use crate::graph::detached::{DetachedNode, DetachedNodeParts};
use crate::graph::error::{
    BindRejection, BindingRejection, DetachReason, DetachedNodeError, GhostSubscriptionRejection,
    GraphValidationError, SubscriptionRejection, Unresolved,
};
use crate::graph::func::Func;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, GhostWiring, InputState, SubscriptionState,
};
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::{EventPort, FuncId, GraphId, PortId};
use crate::graph::manifest::Manifest;
use crate::graph::node::{BodySource, CacheMode, Keep, Node, NodeKind};
use crate::graph::ports::{InputDecl, Numbered};
use crate::graph::reconciled::Repair;
use crate::graph::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort, ShownPort};
use crate::graph::subscription::Subscription;
use crate::graph::wiring::Wiring;
use crate::graph::{Detaching, Graph, InputPort, NodeId, OutputPort};
use crate::internals;
use crate::internals::test_graph::TestGraph;
use common::{ErrorChain, SerdeFormat};
use std::panic;
use std::panic::AssertUnwindSafe;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn validate_passes_for_valid_graph() {
    assert!(TestGraph::sample().graph().validate().is_ok());
}

/// Each mode's RAM and disk answers, hand-tabulated: RAM keeps an output unless it keeps it
/// never, and disk saves it where the bit is set; a join keeps RAM as long as either and disk
/// where either does.
#[test]
fn cache_mode_keeps_ram_and_disk_as_its_fields_say() {
    let mode = |ram, disk| CacheMode { ram, disk };
    let table = [
        (mode(Keep::Never, false), false, false),
        (mode(Keep::Auto, false), true, false),
        (mode(Keep::Always, false), true, false),
        (mode(Keep::Never, true), false, true),
        (mode(Keep::Always, true), true, true),
    ];
    for (mode, ram, disk) in table {
        assert_eq!(mode.caches_in_ram(), ram, "{mode:?} RAM");
        assert_eq!(mode.persists_to_disk(), disk, "{mode:?} disk");
    }
    assert_eq!(CacheMode::default(), CacheMode::AUTO);
    assert_eq!(CacheMode::NEVER.join(CacheMode::AUTO), CacheMode::AUTO);
    assert_eq!(
        CacheMode::ALWAYS.join(mode(Keep::Never, true)),
        mode(Keep::Always, true)
    );
    assert_eq!(CacheMode::NEVER.join(CacheMode::NEVER), CacheMode::NEVER);
}

#[test]
fn cache_mode_round_trips() {
    for mode in [
        CacheMode::NEVER,
        CacheMode::AUTO,
        CacheMode::ALWAYS,
        CacheMode {
            ram: Keep::Never,
            disk: true,
        },
        CacheMode {
            ram: Keep::Always,
            disk: true,
        },
    ] {
        let mut g = TestGraph::new();
        g.add("src", |n| n.pure().output(DataType::Int));
        g.cache("src", mode);

        for format in SerdeFormat::ALL {
            let bytes = format.serialize(&g.graph()).unwrap();
            let back: Graph = format.deserialize(&bytes).unwrap();
            assert_eq!(
                back.find(g.id("src")).unwrap().cache(),
                Some(mode),
                "{mode:?} via {format:?}"
            );
        }
    }
}

#[test]
fn a_new_node_takes_what_its_declaration_says() {
    use crate::graph::node::special::SpecialNode;

    // A fresh func node inherits its func's `default_cache_mode` — the out-of-box
    // `AUTO`, or whatever the func's builder set.
    let plain = internals::stub_func(FuncId::unique(), "plain").build();
    assert_eq!(
        Node::from(&plain).cache(),
        Some(CacheMode::AUTO),
        "default func → kept while memory allows"
    );

    let hot = internals::stub_func(FuncId::unique(), "hot")
        .default_cache_mode(CacheMode {
            ram: Keep::Always,
            disk: true,
        })
        .build();
    assert_eq!(
        Node::from(&hot).cache(),
        Some(CacheMode {
            ram: Keep::Always,
            disk: true
        }),
        "func node copies the func's default_cache_mode"
    );
    // `add_func_node` routes through the same `From<&Func>`, so the graph-level
    // constructor propagates it too.
    let mut graph = Graph::default();
    let id = graph.add_func_node(&hot);
    assert_eq!(
        graph.find(id).unwrap().cache(),
        Some(CacheMode {
            ram: Keep::Always,
            disk: true
        })
    );

    assert_eq!(
        Node::from(&hot).name,
        None,
        "and no name of its own: it reads its func's"
    );

    // A special node's declaration is hardcoded, so `From<SpecialNode>` reaches it for the cache
    // mode.
    let special = SpecialNode::RunSinks;
    let node = Node::from(special);
    assert_eq!(node.name, None);
    assert_eq!(node.cache(), Some(special.func().default_cache_mode()));

    // Resolving a `Func` kind takes a library this constructor does not have, so it starts
    // unnamed and on the default cache: an empty name means a node whose func nothing has
    // resolved.
    let unresolved = internals::unresolved_node(FuncId::unique());
    assert_eq!(unresolved.cache(), Some(CacheMode::AUTO));
    assert_eq!(unresolved.name, None);
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
        g.graph().validate_with(g.scope())
    };

    assert!(
        validate(false).is_ok(),
        "a normal input accepts a wired binding"
    );
    let err = validate(true).expect_err("a const-only input must reject a wired binding");
    assert!(
        matches!(
            err,
            GraphValidationError::RejectedBinding {
                rejection: BindingRejection::WiredConstOnly,
                ..
            }
        ),
        "unexpected error: {err}"
    );
}

/// The type half of admission: an `Int` output feeds an `Int` input and not a
/// `String` one; a `String` literal never fits an `Int` input, a numeric one
/// does (scalar coercion), and the two path shapes fit only their own file
/// mode.
#[test]
fn admit_holds_wires_and_consts_to_the_input_type() {
    let mut g = TestGraph::new();
    g.add("int_src", |n| n.pure().output(DataType::Int));
    g.add("str_sink", |n| n.sink().input(DataType::String));
    g.add("int_sink", |n| n.sink().input(DataType::Int));
    let wire = Binding::wire(g.id("int_src"), PortId::new(1));
    let admit = |sink: &str| {
        g.graph()
            .admit(g.scope(), InputPort::new(g.id(sink), PortId::new(1)), &wire)
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
        let port = InputPort::new(g.id("sink"), PortId::new(1));
        g.graph().admit(g.scope(), port, &Binding::Const(value))
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

    // `get_a → sum.0`, `get_b → sum.1` and `sum → mult.0` touch it; `get_b → mult.1` and
    // `mult → Print` do not.
    let mut survivors = vec![
        (g.input("mult", 1), g.output("get_b", 0)),
        (g.input("Print", 0), g.output("mult", 0)),
    ];
    survivors.sort_unstable();
    g.remove("sum");

    assert!(g.graph().find(sum).is_none());
    assert_eq!(g.graph().len(), 4);
    assert_eq!(
        g.graph().wiring().wires().collect::<Vec<_>>(),
        survivors,
        "exactly the wires that touched it go"
    );
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

    let closes = |from: &str, to: &str| g.graph().produces_cycle(g.id(from), g.id(to));
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

    let closes = |from: &str, to: &str| g.graph().produces_cycle(g.id(from), g.id(to));
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
    let from_port: Binding = OutputPort::new(nid, PortId::new(2)).into();
    assert_eq!(from_port, Binding::wire(nid, PortId::new(2)));
    assert_eq!(
        from_port,
        Binding::Wire(OutputPort::new(nid, PortId::new(2)))
    );

    let from_value: Binding = ConstValue::Int(7).into();
    assert_eq!(from_value, Binding::Const(ConstValue::Int(7)));
}

#[test]
fn input_bindings_are_sparse_and_none_removes_an_entry() {
    let mut g = TestGraph::sample();

    let first = InputPort::new(g.id("sum"), PortId::new(1));
    let second = InputPort::new(g.id("sum"), PortId::new(2));
    let absent = InputPort::new(g.id("sum"), PortId::new(3));
    assert_eq!(
        g.graph().wiring().binding(first),
        Some(&Binding::wire(g.id("get_a"), PortId::new(1)))
    );
    assert_eq!(
        g.graph().wiring().binding(second),
        Some(&Binding::wire(g.id("get_b"), PortId::new(1)))
    );
    assert!(g.graph().wiring().binding(absent).is_none());

    let binding_count = g.graph().wiring().bindings().len();
    g.unbind("sum", 0);
    assert!(g.graph().wiring().binding(first).is_none());
    assert_eq!(g.graph().wiring().bindings().len(), binding_count - 1);
}

#[test]
fn subscribe_unsubscribe_is_subscribed() {
    let mut g = TestGraph::sample();
    let (emitter, sub) = (g.id("get_a"), g.id("sum"));

    assert!(!g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(1)),
        sub
    )));
    g.graph_mut()
        .subscribe(EventPort::new(emitter, PortId::new(1)), sub);
    assert!(g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(1)),
        sub
    )));

    // Distinct event_idx is a distinct edge.
    assert!(!g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(2)),
        sub
    )));

    // Re-subscribing is idempotent (BTreeSet dedups).
    g.graph_mut()
        .subscribe(EventPort::new(emitter, PortId::new(1)), sub);
    assert_eq!(g.graph().wiring().subscriptions().count(), 1);

    // One event carries several subscribers, and a second event off the same
    // emitter is its own edge — so the three coexist rather than overwriting.
    let (sub2, other) = (g.id("mult"), g.id("Print"));
    g.graph_mut()
        .subscribe(EventPort::new(emitter, PortId::new(1)), sub2);
    g.graph_mut()
        .subscribe(EventPort::new(emitter, PortId::new(2)), other);
    assert!(g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(1)),
        sub
    )));
    assert!(g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(1)),
        sub2
    )));
    assert!(g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(2)),
        other
    )));
    assert!(!g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(2)),
        sub
    )));

    // `subscriptions` yields them in (event, subscriber) order, whatever
    // order they were authored in — the determinism a compile's subscriber
    // lists inherit.
    let mut expected = vec![
        Subscription::new(EventPort::new(emitter, PortId::new(1)), sub),
        Subscription::new(EventPort::new(emitter, PortId::new(1)), sub2),
        Subscription::new(EventPort::new(emitter, PortId::new(2)), other),
    ];
    expected.sort();
    assert_eq!(
        g.graph().wiring().subscriptions().collect::<Vec<_>>(),
        expected
    );

    // Dropping one edge leaves its siblings alone.
    g.graph_mut()
        .unsubscribe(EventPort::new(emitter, PortId::new(1)), sub);
    assert!(!g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(1)),
        sub
    )));
    assert!(g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(1)),
        sub2
    )));
    assert!(g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(emitter, PortId::new(2)),
        other
    )));

    g.graph_mut()
        .unsubscribe(EventPort::new(emitter, PortId::new(1)), sub2);
    g.graph_mut()
        .unsubscribe(EventPort::new(emitter, PortId::new(2)), other);
    assert_eq!(g.graph().wiring().subscriptions().count(), 0);
}

/// A detached node carries exactly the wiring that touched it, and attaching it
/// restores the graph. `DetachedNode::new` puts a record from outside in the
/// graph's order and refuses one that could not have come out of a graph; a
/// record that could has to meet a graph it fits, or the attach panics with the
/// graph untouched. A nil id is simply absent from every lookup.
#[test]
fn a_detached_node_restores_its_wiring_and_malformed_records_are_refused() {
    let mut g = TestGraph::sample();
    assert!(g.graph().find(NodeId::nil()).is_none());
    assert!(g.library.by_id(FuncId::nil()).is_none());
    let sum = g.id("sum");
    let get_a = g.id("get_a");
    let mult = g.id("mult");
    // A subscription that touches `sum`, so both kinds of wiring travel.
    g.graph_mut()
        .subscribe(EventPort::new(get_a, PortId::new(1)), sum);
    // Its own two inputs, in port-id order; the binding that reads it is not one.
    assert_eq!(
        g.graph()
            .wiring()
            .bindings_on(sum)
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        [
            InputPort::new(sum, PortId::new(1)),
            InputPort::new(sum, PortId::new(2))
        ]
    );

    let before = g.graph().clone();
    let edges_before = g.graph().wiring().wires().count();
    let detached = g.remove("sum");
    assert_eq!(g.graph().wiring().wires().count(), edges_before - 3);
    assert!(!g.graph().wiring().is_subscribed(Subscription::new(
        EventPort::new(get_a, PortId::new(1)),
        sum
    )));
    assert_eq!(detached.wiring().bindings().len(), 3);
    assert_eq!(detached.wiring().subscriptions().len(), 1);

    let rebuild_with = |wiring, ghost_wiring| {
        DetachedNode::new(DetachedNodeParts {
            node_id: sum,
            node: detached.node().clone(),
            wiring,
            ghost_wiring,
        })
    };
    let rebuild = |wiring| rebuild_with(wiring, GhostWiring::default());
    assert_eq!(
        rebuild(detached.wiring().clone()),
        Ok(detached.clone()),
        "a record from outside is taken as it is"
    );

    let foreign_port = InputPort::new(mult, PortId::new(2));
    let mut foreign = Wiring::default();
    foreign.bind(foreign_port, Binding::Const(1i64.into()));
    let (first, _) = detached.wiring().bindings().next().unwrap();
    let mut bound_first = Wiring::default();
    bound_first.bind(first, Binding::Const(1i64.into()));
    let ghost_on = |port| GhostWiring {
        bindings: [(port, GhostBinding::from(Binding::Const(1i64.into())))].into(),
        ..GhostWiring::default()
    };
    let elsewhere = Subscription::new(EventPort::new(get_a, PortId::new(1)), mult);
    let mut subscribed_elsewhere = Wiring::default();
    subscribed_elsewhere.subscribe(elsewhere);
    let own = detached.wiring().subscriptions().next().unwrap();
    let mut subscribed_own = Wiring::default();
    subscribed_own.subscribe(own);
    let refusals = [
        (
            DetachedNode::new(DetachedNodeParts {
                node_id: NodeId::nil(),
                node: detached.node().clone(),
                wiring: Wiring::default(),
                ghost_wiring: GhostWiring::default(),
            }),
            DetachedNodeError::NilNodeId,
        ),
        (
            rebuild(foreign),
            DetachedNodeError::ForeignBinding { port: foreign_port },
        ),
        (
            rebuild(subscribed_elsewhere),
            DetachedNodeError::ForeignSubscription {
                subscription: elsewhere,
            },
        ),
        (
            rebuild_with(Wiring::default(), ghost_on(foreign_port)),
            DetachedNodeError::ForeignGhost,
        ),
        (
            rebuild_with(bound_first, ghost_on(first)),
            DetachedNodeError::GhostOnBoundInput { port: first },
        ),
        (
            rebuild_with(
                subscribed_own,
                GhostWiring {
                    subscriptions: [(own, GhostSubscription::default())].into(),
                    ..GhostWiring::default()
                },
            ),
            DetachedNodeError::GhostOfSubscription { subscription: own },
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

    g.graph_mut().attach_node(detached.clone());
    assert_eq!(*g.graph(), before);
    let attached = g.graph().clone();
    let twice = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph_mut().attach_node(detached);
    }));
    assert!(
        twice.is_err(),
        "a node already in the graph is not attached again"
    );
    assert_eq!(
        *g.graph(),
        attached,
        "a refused attach leaves the graph alone"
    );

    // A ghost on an input the record binds is wiring there too: the attach is refused before
    // the node goes in, where it once left a live binding and a ghost on one input.
    let detached = g.graph_mut().detach_node(sum);
    g.graph_mut().set_input_state(
        first,
        InputState::Ghost(GhostBinding::from(Binding::Const(1i64.into()))),
    );
    let ghosted = g.graph().clone();
    let over_ghost = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph_mut().attach_node(detached);
    }));
    assert!(
        over_ghost.is_err(),
        "a ghost where the record binds refuses it"
    );
    assert_eq!(*g.graph(), ghosted, "and the graph is untouched");
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
        g.graph().find(id).unwrap().kind,
        NodeKind::Body {
            source: BodySource::Func(func.id()),
            cache: CacheMode::NEVER,
        }
    );
    assert_eq!(
        g.graph()
            .wiring()
            .binding(InputPort::new(id, PortId::new(1))),
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
        g.graph()
            .wiring()
            .binding(InputPort::new(sum, PortId::new(1)))
            .is_none()
    );
    assert!(
        g.graph()
            .wiring()
            .binding(InputPort::new(sum, PortId::new(2)))
            .is_none()
    );
}

#[test]
fn serialization_round_trips_a_graph_through_every_format() -> TestResult {
    let graph = TestGraph::sample().graph().clone();
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
        wiring: WireWiring,
        ghost_nodes: Box<RawValue>,
        ghost_wiring: WireGhosts,
    }
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct WireWiring {
        bindings: Vec<Box<RawValue>>,
        subscriptions: Box<RawValue>,
    }
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct WireGhosts {
        bindings: Vec<Box<RawValue>>,
        subscriptions: Box<RawValue>,
    }

    let mut g = TestGraph::sample();
    let sum = g.id("sum");
    g.graph_mut().set_input_binding(
        InputPort::new(sum, PortId::new(1)),
        Binding::wire(NodeId::unique(), PortId::new(1)),
    );
    let bytes = SerdeFormat::Bitcode.serialize(&g.graph()).unwrap();
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
    let mut nil_ghost = Graph::default();
    nil_ghost.ghost_nodes.insert(
        NodeId::nil(),
        GhostNode {
            node: internals::unresolved_node(FuncId::unique()),
            reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            saved: None,
        },
    );
    assert!(matches!(
        nil_ghost.validate(),
        Err(GraphValidationError::NilNodeId)
    ));

    // Bindings decode from a sequence into a map, so a repeated input port is
    // caught during decode rather than by validation after it.
    let encoded = ser::to_string(&TestGraph::sample().graph()).unwrap();
    let mut wire: WireGraph = ron::from_str(&encoded).expect("the wire shape still matches");
    wire.wiring.bindings.push(wire.wiring.bindings[0].clone());
    let bytes = ser::to_string(&wire).unwrap().into_bytes();
    let error = SerdeFormat::Ron
        .deserialize::<Graph>(&bytes)
        .expect_err("a duplicate input port cannot decode into the binding map");
    let message = ErrorChain::new(&error).to_string();
    assert!(
        message.contains("duplicate binding for input port"),
        "{message}"
    );

    // Ghost bindings decode from a sequence into a map too, so a repeated input is caught in the
    // decode; a ghost node under a node's id decodes, and validation names it.
    let mut ghosted = TestGraph::sample();
    let get_a = ghosted.id("get_a");
    let node = ghosted.graph().find(get_a).unwrap().clone();
    ghost(&mut ghosted, "get_a", DetachReason::TooLarge);
    let encoded = ser::to_string(ghosted.graph()).unwrap();
    let mut wire: WireGraph = ron::from_str(&encoded).expect("the wire shape still matches");
    wire.ghost_wiring
        .bindings
        .push(wire.ghost_wiring.bindings[0].clone());
    let bytes = ser::to_string(&wire).unwrap().into_bytes();
    let error = SerdeFormat::Ron
        .deserialize::<Graph>(&bytes)
        .expect_err("a repeated input cannot decode into the ghost bindings");
    let message = ErrorChain::new(&error).to_string();
    assert!(
        message.contains("duplicate ghost binding for input port"),
        "{message}"
    );
    let mut shadowed = ghosted.graph().clone();
    shadowed.nodes.insert(get_a, node);
    assert!(matches!(
        shadowed.validate(),
        Err(GraphValidationError::GhostHoldsNode { node_id }) if node_id == get_a
    ));
}

/// Make a ghost node of `name`, as a load does of a node its scope refuses.
fn ghost(g: &mut TestGraph, name: &str, reason: DetachReason) -> NodeId {
    let node_id = g.id(name);
    g.graph_mut().detach_into_ghosts(
        vec![Detaching {
            node_id,
            reason,
            saved: None,
        }],
        &mut Vec::new(),
    );
    node_id
}

/// A wire between two ghost nodes is kept once among the graph's ghosts, so either of them
/// returning alone leaves it a ghost and makes live the wires whose other end is a node; the
/// other returning then restores the rest, and the graph is the one it was. Before, the ghost
/// detached first held the wire and lost it when it returned alone.
#[test]
fn a_wire_between_two_ghost_nodes_outlives_either_returning_alone() {
    for first in ["sum", "mult"] {
        let mut g = TestGraph::sample();
        let original = g.graph().clone();
        let [get_a, get_b, sum, mult, print] =
            ["get_a", "get_b", "sum", "mult", "Print"].map(|name| g.id(name));
        let func_of = |g: &TestGraph, id| g.graph().find(id).unwrap().func_id().unwrap();
        let mut funcs = [("sum", sum), ("mult", mult)].map(|(name, id)| {
            let func = g.library.remove(func_of(&g, id)).unwrap();
            (name, id, Some(func))
        });
        let missing = DetachReason::Unresolved(Unresolved::MissingFunc);

        let mut ghosted = [sum, mult];
        ghosted.sort_unstable();
        assert_eq!(
            g.graphs
                .reconcile(&g.library, &Manifest::default())
                .of(GraphId::Main),
            ghosted.map(|node_id| Repair::GhostedNode {
                node_id,
                reason: missing
            })
        );
        let port = |node, number| InputPort::new(node, PortId::new(number));
        let between = (port(mult, 1), Binding::wire(sum, PortId::new(1)));
        let of_sum = [
            (port(sum, 1), Binding::wire(get_a, PortId::new(1))),
            (port(sum, 2), Binding::wire(get_b, PortId::new(1))),
        ];
        let of_mult = [
            (port(mult, 2), Binding::wire(get_b, PortId::new(1))),
            (port(print, 1), Binding::wire(mult, PortId::new(1))),
        ];
        let mut all: Vec<(InputPort, Binding)> = of_sum
            .iter()
            .chain(&of_mult)
            .chain([&between])
            .cloned()
            .collect();
        all.sort_unstable_by_key(|(port, _)| *port);
        let ghosts = |g: &TestGraph| {
            g.graph()
                .ghost_wiring()
                .bindings()
                .map(|(port, ghost)| (port, ghost.binding()))
                .collect::<Vec<_>>()
        };
        assert_eq!(ghosts(&g), all, "each wire is a ghost once");
        assert_eq!(g.graph().wiring().bindings().len(), 0);
        g.graph().validate_with(g.scope()).unwrap();

        let mut restore = |g: &mut TestGraph, name: &str, wires: &[(InputPort, Binding)]| {
            let (_, node_id, func) = funcs.iter_mut().find(|(held, ..)| *held == name).unwrap();
            g.library.add(func.take().unwrap());
            // The node comes back, then each ghost on it in the order the graph keeps them.
            let mut wires = wires.to_vec();
            wires.sort_unstable_by_key(|(port, _)| *port);
            let mut restored: Vec<Repair> = wires
                .into_iter()
                .map(|(port, binding)| Repair::RestoredBinding { port, binding })
                .collect();
            restored.insert(0, Repair::Reattached { node_id: *node_id });
            assert_eq!(
                g.graphs
                    .reconcile(&g.library, &Manifest::default())
                    .of(GraphId::Main),
                restored,
                "{name} returns"
            );
            g.graph().validate_with(g.scope()).unwrap();
        };
        let (alone, then, other) = match first {
            "sum" => (
                &of_sum[..],
                [of_mult.as_slice(), std::slice::from_ref(&between)].concat(),
                "mult",
            ),
            _ => (
                &of_mult[..],
                [of_sum.as_slice(), std::slice::from_ref(&between)].concat(),
                "sum",
            ),
        };
        restore(&mut g, first, alone);
        assert_eq!(ghosts(&g).len(), 3);
        assert!(
            ghosts(&g).contains(&between),
            "the wire between the two stays a ghost while {other} is one"
        );
        restore(&mut g, other, &then);
        assert_eq!(*g.graph(), original, "{first} first");
    }
}

/// A load repairs the ghosts a file breaks: a ghost node saved under a node's id goes and the
/// node stays; a ghost saved on a bound input goes; a ghost that names a node the graph holds as
/// neither goes; and live wiring that names a ghost node becomes a ghost. Live wiring is repaired
/// first, so on `Print`'s input, wired live from a node the file lacks and as a ghost from one it
/// keeps, the live wire goes and the ghost stays. A ghost saved of a
/// subscription the graph has made goes the same way
/// ([`a_ghost_is_held_to_admission_and_the_loop_check`]).
#[test]
fn a_load_repairs_the_ghosts_a_file_breaks() {
    let mut g = TestGraph::sample();
    let [get_a, get_b, sum, mult, print] =
        ["get_a", "get_b", "sum", "mult", "Print"].map(|name| g.id(name));
    let missing = DetachReason::Unresolved(Unresolved::MissingFunc);
    let sum_func = g.graph().find(sum).unwrap().func_id().unwrap();
    g.library.remove(sum_func).unwrap();
    let port = |node, number| InputPort::new(node, PortId::new(number));
    let absent = NodeId::unique();
    let stray = Subscription::new(EventPort::new(absent, PortId::new(1)), mult);
    let five = Binding::Const(ConstValue::Int(5));
    {
        let graph = g.graph_mut();
        // `sum` as a ghost node with its wiring left live, and a ghost node under `get_a`'s id.
        let node = graph.nodes.remove(&sum).unwrap();
        let ghost_node = GhostNode {
            node,
            reason: missing,
            saved: None,
        };
        graph.ghost_nodes.insert(get_a, ghost_node.clone());
        graph.ghost_nodes.insert(sum, ghost_node);
        let ghosts = &mut graph.ghost_wiring;
        ghosts
            .bindings
            .insert(port(mult, 2), GhostBinding::from(five.clone()));
        ghosts
            .bindings
            .insert(port(absent, 1), GhostBinding::from(five.clone()));
        ghosts
            .subscriptions
            .insert(stray, GhostSubscription::default());
        // A ghost on `sum`'s first input of another binding than the live one, which the live
        // one replaces as it becomes a ghost; one on its second of the same binding, which stays.
        ghosts
            .bindings
            .insert(port(sum, 1), GhostBinding::from(five.clone()));
        ghosts.bindings.insert(
            port(sum, 2),
            GhostBinding::from(Binding::wire(get_b, PortId::new(1))),
        );
        // An output `get_b` does not declare, so the load keeps the ghost rather than restore it.
        ghosts.bindings.insert(
            port(print, 1),
            GhostBinding::from(Binding::wire(get_b, PortId::new(9))),
        );
        graph
            .wiring
            .bind(port(print, 1), Binding::wire(absent, PortId::new(1)));
    }

    let reconciled = g
        .graphs
        .reconcile(&g.library, &Manifest::default())
        .of(GraphId::Main);

    let into_ghost = |port, binding| Repair::GhostedBinding {
        port,
        binding,
        rejection: BindRejection::GhostNode { node_id: sum },
    };
    let mut ghost_repairs = vec![
        Repair::GhostOnBoundInput {
            port: port(mult, 2),
            binding: five.clone(),
        },
        Repair::DanglingBinding {
            port: port(absent, 1),
            binding: five.clone(),
        },
    ];
    ghost_repairs.sort_by_key(|repair| match repair {
        Repair::GhostOnBoundInput { port, .. } | Repair::DanglingBinding { port, .. } => *port,
        _ => unreachable!(),
    });
    let mut live_repairs = vec![
        Repair::GhostOnBoundInput {
            port: port(sum, 1),
            binding: five,
        },
        into_ghost(port(sum, 1), Binding::wire(get_a, PortId::new(1))),
        into_ghost(port(sum, 2), Binding::wire(get_b, PortId::new(1))),
        into_ghost(port(mult, 1), Binding::wire(sum, PortId::new(1))),
        Repair::DanglingBinding {
            port: port(print, 1),
            binding: Binding::wire(absent, PortId::new(1)),
        },
    ];
    // Stable, so the ghost a live binding replaces is reported before the binding it became.
    live_repairs.sort_by_key(|repair| match repair {
        Repair::GhostedBinding { port, .. }
        | Repair::DanglingBinding { port, .. }
        | Repair::GhostOnBoundInput { port, .. } => *port,
        _ => unreachable!(),
    });
    // The live wiring first, so a ghost is weighed against the live wiring that stays.
    let expected: Vec<Repair> = [Repair::DuplicateGhostNode { node_id: get_a }]
        .into_iter()
        .chain(live_repairs)
        .chain(ghost_repairs)
        .chain([Repair::DanglingSubscription {
            subscription: stray,
        }])
        .collect();
    assert_eq!(reconciled, expected);
    assert!(g.graph().find(get_a).is_some(), "the node keeps its id");
    assert_eq!(
        g.graph()
            .ghost_nodes()
            .map(|(node_id, _)| node_id)
            .collect::<Vec<_>>(),
        [sum]
    );
    assert_eq!(g.graph().ghost_wiring().bindings().len(), 4);
    assert_eq!(
        g.graph()
            .ghost_wiring()
            .binding(port(print, 1))
            .and_then(GhostBinding::output),
        Some(OutputPort::new(get_b, PortId::new(9)))
    );
    assert_eq!(g.graph().ghost_wiring().subscriptions().len(), 0);
    g.graph().validate_with(g.scope()).unwrap();
    assert!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main)
            .is_empty()
    );
}

/// A graph keeps each node it cannot hold as a ghost node beside its nodes: the ids of the two
/// are disjoint, so neither an insert nor a ghost node takes an id the other holds; the ghosts
/// round-trip with the graph; the constants of the graph are its bindings' and its ghosts', for
/// reading and for writing; and a ghost node comes out with each ghost that touches it and goes
/// back as it was.
#[test]
fn a_graph_keeps_its_ghosts_beside_its_nodes() {
    let mut g = TestGraph::sample();
    g.constant("sum", 1, 4i64);
    let (get_a, get_b, sum, mult) = (g.id("get_a"), g.id("get_b"), g.id("sum"), g.id("mult"));
    let node = g.graph().find(sum).unwrap().clone();
    let reason = DetachReason::Unresolved(Unresolved::MissingFunc);
    ghost(&mut g, "sum", reason);
    let graph = g.graph().clone();
    assert!(graph.find(sum).is_none());
    assert_eq!(
        graph.ghost_node(sum),
        Some(&GhostNode {
            node: node.clone(),
            reason,
            saved: None
        })
    );
    assert!(graph.holds(sum));
    assert!(graph.holds(get_b));
    assert!(!graph.holds(NodeId::unique()));
    assert_eq!(graph.len(), 4, "a ghost node is no node");
    graph.validate().unwrap();

    let node_over_ghost = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph_mut().insert(sum, node.clone());
    }));
    assert!(node_over_ghost.is_err(), "a node takes no ghost node's id");
    let ghost_over_node = panic::catch_unwind(AssertUnwindSafe(|| {
        g.graph_mut().insert_ghost_node(
            get_b,
            GhostNode {
                node: node.clone(),
                reason,
                saved: None,
            },
        );
    }));
    assert!(ghost_over_node.is_err(), "a ghost node takes no node's id");

    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&graph).unwrap();
        assert_eq!(
            format.deserialize::<Graph>(&bytes).unwrap(),
            graph,
            "{format:?}"
        );
    }

    let port = |node, number| InputPort::new(node, PortId::new(number));
    let mut wires: Vec<_> = graph.ghost_wiring().wires().collect();
    wires.sort_unstable();
    let mut expected = [
        (port(sum, 1), OutputPort::new(get_a, PortId::new(1))),
        (port(mult, 1), OutputPort::new(sum, PortId::new(1))),
    ];
    expected.sort_unstable();
    assert_eq!(
        wires, expected,
        "each wire that touched it, from either end"
    );
    assert_eq!(
        graph.constants().collect::<Vec<_>>(),
        [(port(sum, 2), &ConstValue::Int(4))],
        "the sample's only constant is a ghost"
    );
    let mut written = graph.clone();
    for value in written.constants_mut() {
        *value = ConstValue::Int(9);
    }
    assert_eq!(
        written.input_state(port(sum, 2)),
        InputState::Ghost(GhostBinding::from(Binding::Const(ConstValue::Int(9))))
    );
    written.validate().unwrap();

    // A node comes out with the ghosts that touch it, and goes back with them.
    let live = g.graph_mut().detach_node(get_a);
    assert_eq!(
        live.ghost_wiring()
            .wires()
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        [port(sum, 1)]
    );
    assert_eq!(g.graph().ghost_wiring().bindings().len(), 2);
    g.graph().validate().unwrap();
    g.graph_mut().attach_node(live);
    assert_eq!(*g.graph(), graph);

    let detached = g.graph_mut().detach_ghost_node(sum);
    assert_eq!(g.graph().snapshot_ghost_node(sum), None);
    assert_eq!((detached.node_id(), detached.ghost().reason), (sum, reason));
    assert_eq!(detached.wiring().bindings().len(), 3);
    assert!(g.graph().ghost_wiring().is_empty());
    assert!(!g.graph().holds(sum));
    g.graph().validate().unwrap();
    g.graph_mut().attach_ghost_node(detached);
    assert_eq!(*g.graph(), graph);
}

/// An input holds one binding, live or ghost: a write of either takes the other's place, and a
/// subscription is made or kept as a ghost, never both.
#[test]
fn an_input_holds_one_binding_live_or_ghost() {
    let mut g = TestGraph::sample();
    let (get_a, get_b, sum) = (g.id("get_a"), g.id("get_b"), g.id("sum"));
    let port = InputPort::new(sum, PortId::new(1));
    let wire = Binding::wire(get_a, PortId::new(1));
    assert_eq!(g.graph().input_state(port), InputState::Live(wire.clone()));

    let kept = GhostBinding::from(wire);
    g.graph_mut()
        .set_input_state(port, InputState::Ghost(kept.clone()));
    assert_eq!(g.graph().wiring().binding(port), None);
    assert_eq!(g.graph().ghost_wiring().binding(port), Some(&kept));
    assert_eq!(g.graph().input_state(port), InputState::Ghost(kept));

    let other = Binding::wire(get_b, PortId::new(1));
    g.graph_mut().set_input_binding(port, other.clone());
    assert_eq!(g.graph().input_state(port), InputState::Live(other));
    assert!(
        g.graph().ghost_wiring().is_empty(),
        "the bind replaced the ghost"
    );
    g.graph_mut().set_input_state(port, InputState::Unbound);
    assert_eq!(g.graph().input_state(port), InputState::Unbound);
    assert_eq!(g.graph().wiring().binding(port), None);

    let subscription = Subscription::new(EventPort::new(get_a, PortId::new(1)), sum);
    let state = |g: &TestGraph| g.graph().subscription_state(subscription);
    assert_eq!(state(&g), SubscriptionState::Absent);
    let ghost = SubscriptionState::Ghost(GhostSubscription::default());
    g.graph_mut()
        .set_subscription_state(subscription, ghost.clone());
    assert_eq!(state(&g), ghost);
    assert!(
        !g.graph()
            .wiring()
            .is_subscribed(Subscription::new(subscription.event, sum))
    );
    g.graph_mut().subscribe(subscription.event, sum);
    assert_eq!(state(&g), SubscriptionState::Live);
    assert_eq!(g.graph().ghost_wiring().subscriptions().len(), 0);
    g.graph_mut().unsubscribe(subscription.event, sum);
    assert_eq!(state(&g), SubscriptionState::Absent);
    g.graph().validate().unwrap();
}

/// A ghost wire that would close a loop stays a ghost while the loop stands, says so when asked,
/// and a load makes it live once the wire that completes the loop is gone. A ghost whose end is
/// a ghost node names the node; a ghost subscription is held to the same two questions.
#[test]
fn a_ghost_is_held_to_admission_and_the_loop_check() {
    let mut g = TestGraph::new();
    g.add("a", |n| n.pure().input(DataType::Int).output(DataType::Int));
    g.instance("b", "a");
    g.add("ticker", |n| {
        n.output(DataType::Int)
            .event("tick", internals::stub_event())
    });
    g.add("sink", |n| n.sink().optional(DataType::Int));
    g.wire("a", "b");
    g.wire("b", "a");
    g.wire("ticker", "sink");
    g.subscribe("ticker", 0, "sink");
    let [a, b, ticker, sink] = ["a", "b", "ticker", "sink"].map(|name| g.id(name));
    let closing = InputPort::new(b, PortId::new(1));
    let from_a = Binding::wire(a, PortId::new(1));
    assert_eq!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main),
        [Repair::GhostedBinding {
            port: closing,
            binding: from_a.clone(),
            rejection: BindRejection::Cycle,
        }],
        "the walk enters `a` first, reads `b`, and `b`'s wire from `a` returns to the path"
    );
    assert_eq!(
        g.graph().admit_ghost(g.scope(), closing),
        Err(BindRejection::Cycle)
    );
    assert!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main)
            .is_empty()
    );

    g.unbind("a", 0);
    assert_eq!(g.graph().admit_ghost(g.scope(), closing), Ok(()));
    assert_eq!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main),
        [Repair::RestoredBinding {
            port: closing,
            binding: from_a.clone(),
        }]
    );
    assert_eq!(g.graph().wiring().binding(closing), Some(&from_a));

    let into_sink = InputPort::new(sink, PortId::new(1));
    let tick = Subscription::new(EventPort::new(ticker, PortId::new(1)), sink);
    ghost(&mut g, "ticker", DetachReason::TooLarge);
    assert_eq!(
        g.graph().admit_ghost(g.scope(), into_sink),
        Err(BindRejection::GhostNode { node_id: ticker })
    );
    assert_eq!(
        g.graph().admit_ghost_subscription(g.scope(), tick),
        Err(GhostSubscriptionRejection::GhostNode { node_id: ticker })
    );
    assert_eq!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main),
        [
            Repair::Reattached { node_id: ticker },
            Repair::RestoredBinding {
                port: into_sink,
                binding: Binding::wire(ticker, PortId::new(1)),
            },
            Repair::RestoredSubscription { subscription: tick },
        ],
        "a ghost node that resolves returns with its wiring"
    );
    assert!(g.graph().ghost_wiring().is_empty());
    assert!(
        g.graph()
            .wiring()
            .is_subscribed(Subscription::new(tick.event, sink))
    );

    // A file that keeps a ghost of a subscription it also makes: the ghost goes.
    g.graph_mut()
        .ghost_wiring
        .subscriptions
        .insert(tick, GhostSubscription::default());
    assert!(matches!(
        g.graph().validate(),
        Err(GraphValidationError::GhostOfSubscription { subscription }) if subscription == tick
    ));
    assert_eq!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main),
        [Repair::GhostOfSubscription { subscription: tick }]
    );
    assert!(
        g.graph()
            .wiring()
            .is_subscribed(Subscription::new(tick.event, sink))
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
    let admit = |number, binding: Binding| {
        g.graph().admit(
            g.scope(),
            InputPort::new(dst, PortId::new(number)),
            &binding,
        )
    };
    assert_eq!(
        admit(6, Binding::Const(ConstValue::Int(1))),
        Err(BindingRejection::UndeclaredInput)
    );
    assert_eq!(
        admit(1, Binding::wire(src, PortId::new(8))),
        Err(BindingRejection::UndeclaredOutput)
    );
    assert_eq!(admit(1, Binding::Const(ConstValue::Null)), Ok(()));
    assert_eq!(
        admit(2, Binding::Const(ConstValue::Null)),
        Err(BindingRejection::ConstMismatch)
    );

    let subscribe = |number, subscriber| {
        let subscription = Subscription::new(EventPort::new(src, PortId::new(number)), subscriber);
        g.graph().admit_subscription(g.scope(), subscription)
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
/// every binding whose port is still declared and still admits it, and makes a ghost of the
/// rest, each reported. A port gained leaves the node's wiring as it was, the new input unbound;
/// a rename and a reorder change nothing; a port that leaves strands its own binding alone; a
/// retype strands what the new type does not take. A ghost is live again at the load that finds
/// its port and type back.
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
        let original = g.graph().clone();
        let before = g.library.by_name("dst").unwrap().clone();
        g.edit_func("dst", edit);
        let report = g
            .graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main);
        assert!(g.graph().find(g.id("dst")).is_some(), "the node resolves");
        let bindings: Vec<(PortId, Binding)> = g
            .graph()
            .wiring()
            .bindings_on(g.id("dst"))
            .map(|(port, binding)| (port.port, binding.clone()))
            .collect();
        let ghosts: Vec<(InputPort, Binding)> = g
            .graph()
            .ghost_wiring()
            .bindings()
            .map(|(port, ghost)| (port, ghost.binding()))
            .collect();
        let kept: Vec<(InputPort, Binding)> = report
            .iter()
            .map(|repair| match repair {
                Repair::GhostedBinding { port, binding, .. } => (*port, binding.clone()),
                other => panic!("a changed func strands bindings alone: {other:?}"),
            })
            .collect();
        assert_eq!(ghosts, kept, "each refused binding is kept as a ghost");

        // The ports as they were: each ghost is live again.
        g.library.remove(before.id()).unwrap();
        g.library.add(before);
        let restored = g
            .graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main);
        assert_eq!(
            restored,
            kept.into_iter()
                .map(|(port, binding)| Repair::RestoredBinding { port, binding })
                .collect::<Vec<_>>()
        );
        assert_eq!(*g.graph(), original);
        (bindings, report)
    };
    let src = fixture().id("src");
    let all = vec![
        (PortId::new(1), Binding::wire(src, PortId::new(1))),
        (PortId::new(2), Binding::Const(ConstValue::Int(2))),
        (PortId::new(3), Binding::Const(ConstValue::Int(3))),
    ];

    let gained: fn(&mut FuncBuilder) = |f| {
        f.ports.inputs.push(Numbered::new(
            PortId::new(4),
            InputDecl::required("new", DataType::Int),
        ));
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
        [Repair::GhostedBinding {
            port: InputPort::new(dst, PortId::new(2)),
            binding: Binding::Const(ConstValue::Int(2)),
            rejection: BindingRejection::UndeclaredInput.into(),
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
            Repair::GhostedBinding {
                port: InputPort::new(dst, PortId::new(1)),
                binding: Binding::wire(src, PortId::new(1)),
                rejection: BindingRejection::WireMismatch.into(),
            },
            Repair::GhostedBinding {
                port: InputPort::new(dst, PortId::new(3)),
                binding: Binding::Const(ConstValue::Int(3)),
                rejection: BindingRejection::ConstMismatch.into(),
            },
        ]
    );
}

/// The manifest of a save is what each func declared then, and a load reads it to tell what
/// changed. A port that left strands what you set on it as a ghost that keeps the port's name
/// and type, and the default nobody set goes; an output and an event that left name themselves
/// on the ghosts that read them. A port gained takes its default as on a new node, and so does
/// an input whose old default the new type refuses, while an input you cleared stays cleared.
/// With no manifest a load knows none of it. A ghost node holds what its func declared, and
/// when the func returns changed it is read as the manifest was.
#[test]
fn a_manifest_tells_a_load_what_each_func_declared() {
    let fixture = || {
        let mut g = TestGraph::new();
        g.add("src", |n| {
            n.output(DataType::Int)
                .event("tick", internals::stub_event())
        });
        g.add("dst", |n| {
            n.sink()
                .input(DataType::Int)
                .optional(DataType::Int)
                .optional(DataType::Int)
        });
        g.edit_func("dst", |f| {
            f.ports.inputs[1].default_value = Some(ConstValue::Int(2));
            f.ports.inputs[2].default_value = Some(ConstValue::Int(3));
        });
        g.wire("src", ("dst", 0));
        g.constant("dst", 1, 2i64);
        g.constant("dst", 2, 7i64);
        g.subscribe("src", 0, "dst");
        g
    };
    let (src, dst) = (fixture().id("src"), fixture().id("dst"));
    let port = |number| InputPort::new(dst, PortId::new(number));
    let from_src = Binding::wire(src, PortId::new(1));
    let tick = Subscription::new(EventPort::new(src, PortId::new(1)), dst);
    let int = |value| Binding::Const(ConstValue::Int(value));
    let names = |g: &TestGraph, name: &str| {
        let func = g.library.by_name(name).unwrap();
        (
            func.id(),
            func.inputs()
                .iter()
                .map(|input| input.name.clone())
                .collect::<Vec<_>>(),
            func.outputs()[..]
                .iter()
                .map(|output| output.name.clone())
                .collect::<Vec<_>>(),
        )
    };
    let (dst_func, dst_inputs, _) = names(&fixture(), "dst");
    let (src_func, _, src_outputs) = names(&fixture(), "src");
    let saved_int = |name: &String| SavedPort {
        name: name.clone(),
        data_type: DataType::Int,
    };

    let manifest = fixture().graphs.manifest(&fixture().library);
    assert_eq!(manifest.iter().map(|(id, _)| id).collect::<Vec<_>>(), {
        let mut ids = [src_func, dst_func];
        ids.sort_unstable();
        ids
    });
    assert_eq!(
        manifest.func(dst_func),
        Some(&SavedDecl {
            name: "dst".to_owned(),
            category: String::new(),
            inputs: [(1, None), (2, Some(2)), (3, Some(3))]
                .into_iter()
                .zip(&dst_inputs)
                .map(|((number, default), name)| {
                    Numbered::new(
                        PortId::new(number),
                        SavedInput {
                            name: name.clone(),
                            data_type: DataType::Int,
                            default_value: default.map(ConstValue::Int),
                        },
                    )
                })
                .collect(),
            outputs: vec![],
            events: vec![],
        })
    );
    assert_eq!(
        manifest.func(src_func).map(|saved| &saved.events),
        Some(&vec![Numbered::new(
            PortId::new(1),
            SavedEvent {
                name: "tick".to_owned()
            }
        )])
    );
    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&manifest).unwrap();
        assert_eq!(
            format.deserialize::<Manifest>(&bytes).unwrap(),
            manifest,
            "{format:?}"
        );
    }

    // Ports that left: an output and an event of `src` renumbered, inputs 2 and 3 of `dst` gone.
    let mut g = fixture();
    g.edit_func("src", |f| {
        f.ports.outputs[0].id = PortId::new(2);
        f.events[0].id = PortId::new(5);
    });
    g.edit_func("dst", |f| f.ports.inputs.truncate(1));
    let ghosted = |number, binding, rejection: BindingRejection| Repair::GhostedBinding {
        port: port(number),
        binding,
        rejection: rejection.into(),
    };
    assert_eq!(
        g.graphs.reconcile(&g.library, &manifest).of(GraphId::Main),
        [
            ghosted(1, from_src.clone(), BindingRejection::UndeclaredOutput),
            ghosted(2, int(2), BindingRejection::UndeclaredInput),
            ghosted(3, int(7), BindingRejection::UndeclaredInput),
            Repair::GhostedSubscription {
                subscription: tick,
                rejection: SubscriptionRejection::UndeclaredEvent.into(),
            },
            Repair::DefaultDropped {
                port: port(2),
                value: ConstValue::Int(2),
            },
        ]
    );
    let ghosts = g.graph().ghost_wiring();
    assert_eq!(
        ghosts.bindings().collect::<Vec<_>>(),
        [
            (
                port(1),
                &GhostBinding {
                    input: None,
                    source: GhostSource::Wire {
                        output: OutputPort::new(src, PortId::new(1)),
                        saved: Some(saved_int(&src_outputs[0])),
                    },
                }
            ),
            (
                port(3),
                &GhostBinding {
                    input: Some(saved_int(&dst_inputs[2])),
                    source: GhostSource::Const(ConstValue::Int(7)),
                }
            ),
        ],
        "the default nobody set is gone, and each lost port names itself on its ghost"
    );
    assert_eq!(
        ghosts.subscription(tick),
        Some(&GhostSubscription {
            event: Some(SavedEvent {
                name: "tick".to_owned()
            }),
            trigger: None,
        })
    );
    let saved_again = g.graphs.manifest(&g.library);
    assert!(
        g.graphs.reconcile(&g.library, &saved_again).is_empty(),
        "a second load of the saved set is clean"
    );

    // A port gained, a default the new type refuses, and an input cleared before the save.
    let changed = |manifest: &Manifest| {
        let mut g = fixture();
        g.unbind("dst", 2);
        g.edit_func("dst", |f| {
            f.ports.inputs[1].data_type = DataType::String;
            f.ports.inputs[1].default_value = Some(ConstValue::String("x".to_owned()));
            f.ports.inputs.push(Numbered::new(
                PortId::new(4),
                InputDecl::optional("new", DataType::Int).default(9i64),
            ));
        });
        let repairs = g.graphs.reconcile(&g.library, manifest).of(GraphId::Main);
        let states = [2, 3, 4].map(|number| g.graph().input_state(port(number)));
        (repairs, states)
    };
    let refused = ghosted(2, int(2), BindingRejection::ConstMismatch);
    let text = Binding::Const(ConstValue::String("x".to_owned()));
    assert_eq!(
        changed(&manifest),
        (
            vec![
                refused.clone(),
                Repair::DefaultDropped {
                    port: port(2),
                    value: ConstValue::Int(2),
                },
                Repair::DefaultSeeded {
                    port: port(2),
                    binding: text.clone(),
                },
                Repair::DefaultSeeded {
                    port: port(4),
                    binding: int(9),
                },
            ],
            [
                InputState::Live(text),
                InputState::Unbound,
                InputState::Live(int(9)),
            ]
        ),
        "the cleared input 3 stays cleared: the manifest shows it is no new port"
    );
    assert_eq!(
        changed(&Manifest::default()),
        (
            vec![refused],
            [
                InputState::Ghost(GhostBinding::from(int(2))),
                InputState::Unbound,
                InputState::Unbound,
            ]
        ),
        "with no manifest a load keeps the old default and seeds nothing"
    );

    // A func the library lost: the ghost node holds what it declared. When the func returns
    // without input 3 and with a new input 4, the ghost node's saved declaration is read as the
    // manifest was, which no longer lists the func.
    let mut g = fixture();
    let lost = g.library.remove(dst_func).unwrap();
    assert_eq!(
        g.graphs.reconcile(&g.library, &manifest).of(GraphId::Main),
        [Repair::GhostedNode {
            node_id: dst,
            reason: DetachReason::Unresolved(Unresolved::MissingFunc),
        }]
    );
    assert_eq!(
        g.graph().ghost_node(dst).unwrap().saved.as_ref(),
        manifest.func(dst_func)
    );
    let without = g.graphs.manifest(&g.library);
    assert_eq!(without.func(dst_func), None, "a ghost node holds its own");
    assert!(g.graphs.reconcile(&g.library, &without).is_empty());
    let mut returned = lost.into_builder();
    returned.ports.inputs.truncate(2);
    returned.ports.inputs.push(Numbered::new(
        PortId::new(4),
        InputDecl::optional("new", DataType::Int).default(9i64),
    ));
    g.library.add(returned.build());
    assert_eq!(
        g.graphs.reconcile(&g.library, &without).of(GraphId::Main),
        [
            Repair::Reattached { node_id: dst },
            Repair::RestoredBinding {
                port: port(1),
                binding: from_src,
            },
            Repair::RestoredBinding {
                port: port(2),
                binding: int(2),
            },
            Repair::RestoredSubscription { subscription: tick },
            Repair::DefaultSeeded {
                port: port(4),
                binding: int(9),
            },
        ]
    );
    assert_eq!(
        g.graph().input_state(port(3)),
        InputState::Ghost(GhostBinding {
            input: Some(saved_int(&dst_inputs[2])),
            source: GhostSource::Const(ConstValue::Int(7)),
        }),
        "what you set on the input that left is kept under its name"
    );
    g.graph().validate_with(g.scope()).unwrap();

    // A saved port where a declaration gives the port again, as an edit that brings the port
    // back leaves it, is no broken graph: the declaration wins, and the next load clears it.
    let mut stale = g.graph().clone();
    stale.set_input_state(
        port(2),
        InputState::Ghost(GhostBinding {
            input: Some(saved_int(&dst_inputs[1])),
            source: GhostSource::Const(ConstValue::Int(1)),
        }),
    );
    g.graphs = GraphSet::from(stale);
    g.graph().validate_with(g.scope()).unwrap();
    let cleared = g.graphs.reconcile(&g.library, &Manifest::default());
    assert_eq!(
        cleared.of(GraphId::Main),
        [Repair::RestoredBinding {
            port: port(2),
            binding: int(1),
        }],
        "the input takes the constant again, so the load restores it"
    );
}

/// A node resolves to the func under its id, whatever its ports are now; one the library lacks
/// does not.
#[test]
fn resolve_finds_a_nodes_declaration_or_says_why_not() {
    let mut g = TestGraph::new();
    g.add("f", |n| n.pure().input(DataType::Int).output(DataType::Int));
    let id = g.id("f");
    let func_id = g.graph().find(id).unwrap().func_id().unwrap();
    let resolved = |g: &TestGraph| {
        g.scope()
            .resolve(g.graph().find(id).unwrap())
            .map(|decl| decl.func().map(Func::id))
    };
    assert_eq!(resolved(&g), Ok(Some(func_id)));

    g.edit_func("f", |func| {
        func.ports.inputs[0].data_type = DataType::String;
    });
    assert_eq!(resolved(&g), Ok(Some(func_id)));

    let missing = internals::unresolved_node(FuncId::from_u128(99));
    assert_eq!(
        g.scope()
            .resolve(&missing)
            .map(|decl| decl.func().map(Func::id)),
        Err(Unresolved::MissingFunc)
    );
}

/// A reconcile takes out of the live tables exactly what the library cannot hold, reports each
/// change in a fixed order, keeps as a ghost each node its scope does not resolve and each
/// binding and subscription a library could bring back, removes what none could, and leaves a
/// graph that validates and compiles. `Null` stays on an optional input.
#[test]
fn reconcile_keeps_as_ghosts_what_the_library_cannot_hold() {
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

    let dst_port = |number| InputPort::new(dst, PortId::new(number));
    g.constant("dst", 0, ConstValue::Null);
    g.graph_mut()
        .set_input_binding(dst_port(2), Binding::wire(src, PortId::new(8)));
    g.wire("src", ("dst", 2));
    g.constant("dst", 3, 1i64);
    g.wire("src", ("dst", 4));
    g.graph_mut()
        .set_input_binding(dst_port(6), Binding::Const(ConstValue::Int(1)));
    g.wire("a", "b");
    g.wire("b", "a");
    g.subscribe("src", 0, "dst");
    g.subscribe("src", 0, "src");
    g.graph_mut()
        .subscribe(EventPort::new(src, PortId::new(4)), dst);
    g.subscribe("src", 0, "gone");
    let absent_port = InputPort::new(absent_consumer, PortId::new(1));
    g.graph_mut()
        .set_input_binding(absent_port, Binding::Const(ConstValue::Int(2)));
    let absent_event = EventPort::new(absent_emitter, PortId::new(1));
    g.graph_mut().subscribe(absent_event, dst);
    let src_func = g.library.by_name("src").unwrap();
    g.graphs
        .main_mut()
        .nodes
        .insert(NodeId::nil(), Node::from(src_func));
    // A ghost node under the nil id that would resolve, which the load once attached and
    // panicked on.
    g.graphs.main_mut().ghost_nodes.insert(
        NodeId::nil(),
        GhostNode {
            node: Node::from(src_func),
            reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            saved: None,
        },
    );
    g.graph_mut()
        .insert(nil_func, internals::unresolved_node(FuncId::nil()));
    let gone_func = g.graph().find(gone).unwrap().func_id().unwrap();
    g.library.remove(gone_func);

    let reconciled = g
        .graphs
        .reconcile(&g.library, &Manifest::default())
        .of(GraphId::Main);

    let rejected = |number, binding, rejection: BindingRejection| Repair::GhostedBinding {
        port: dst_port(number),
        binding,
        rejection: rejection.into(),
    };
    assert_eq!(
        reconciled,
        [
            Repair::NilNode,
            Repair::NilNode,
            Repair::DanglingBinding {
                port: absent_port,
                binding: Binding::Const(ConstValue::Int(2)),
            },
            Repair::DanglingSubscription {
                subscription: Subscription::new(absent_event, dst),
            },
            Repair::GhostedNode {
                node_id: gone,
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            },
            Repair::GhostedNode {
                node_id: nil_func,
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            },
            rejected(
                2,
                Binding::wire(src, PortId::new(8)),
                BindingRejection::UndeclaredOutput
            ),
            rejected(
                3,
                Binding::wire(src, PortId::new(1)),
                BindingRejection::WiredConstOnly
            ),
            rejected(
                4,
                Binding::Const(ConstValue::Int(1)),
                BindingRejection::ConstMismatch
            ),
            rejected(
                5,
                Binding::wire(src, PortId::new(1)),
                BindingRejection::WireMismatch
            ),
            rejected(
                6,
                Binding::Const(ConstValue::Int(1)),
                BindingRejection::UndeclaredInput
            ),
            Repair::RejectedSubscription {
                subscription: Subscription::new(EventPort::new(src, PortId::new(1)), src),
                rejection: SubscriptionRejection::SelfSubscription,
            },
            Repair::GhostedSubscription {
                subscription: Subscription::new(EventPort::new(src, PortId::new(4)), dst),
                rejection: SubscriptionRejection::UndeclaredEvent.into(),
            },
            // The walk enters `a` first, reads `b`, and `b`'s wire from `a`
            // returns to the path.
            Repair::GhostedBinding {
                port: InputPort::new(g.id("b"), PortId::new(1)),
                binding: Binding::wire(g.id("a"), PortId::new(1)),
                rejection: BindRejection::Cycle,
            },
        ]
    );

    assert_eq!(
        g.graph()
            .ghost_nodes()
            .map(|(node_id, _)| node_id)
            .collect::<Vec<_>>(),
        [gone, nil_func],
        "each node it detached is a ghost node of the graph"
    );
    let ghosts = g.graph().ghost_wiring();
    let mut kept: Vec<InputPort> = (2..=6).map(dst_port).collect();
    kept.push(InputPort::new(g.id("b"), PortId::new(1)));
    kept.sort_unstable();
    assert_eq!(
        ghosts.bindings().map(|(port, _)| port).collect::<Vec<_>>(),
        kept,
        "each refused binding is a ghost"
    );
    assert_eq!(
        ghosts
            .subscriptions()
            .map(|(subscription, _)| subscription)
            .collect::<Vec<_>>(),
        [
            Subscription::new(EventPort::new(src, PortId::new(1)), gone),
            Subscription::new(EventPort::new(src, PortId::new(4)), dst),
        ],
        "the ghost node's subscription and the one to an event `src` lacks"
    );

    assert_eq!(
        g.graph().wiring().bindings_on(dst).collect::<Vec<_>>(),
        [(
            InputPort::new(dst, PortId::new(1)),
            &Binding::Const(ConstValue::Null)
        )]
    );
    assert_eq!(
        g.graph().wiring().subscriptions().collect::<Vec<_>>(),
        [Subscription::new(EventPort::new(src, PortId::new(1)), dst)]
    );
    assert!(g.graph().validate_with(g.scope()).is_ok());
    assert_eq!(g.compile().program.e_nodes.len(), 4);

    assert!(
        g.graphs
            .reconcile(&g.library, &Manifest::default())
            .of(GraphId::Main)
            .is_empty(),
        "a reconciled graph needs nothing more"
    );
}

/// A graph's wiring answers by node: the bindings on its inputs and the subscriptions from its
/// events, each a contiguous range; with a set of nodes, every piece that touches any of them,
/// once, and taking that out leaves the rest. Built from pairs, a port given twice keeps the last.
#[test]
fn wiring_answers_by_node_and_takes_out_what_touches_a_set() {
    let g = TestGraph::sample();
    let [get_a, get_b, sum, mult, print] =
        ["get_a", "get_b", "sum", "mult", "Print"].map(|name| g.id(name));
    let wiring = g.graph().wiring();
    assert_eq!(
        wiring
            .bindings_on(sum)
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        [g.input("sum", 0), g.input("sum", 1)]
    );
    assert_eq!(
        wiring.bindings_on(get_a).count(),
        0,
        "a source binds nothing"
    );

    // `sum` and `Print` touch every wire but `get_b → mult.1`.
    let mut nodes = [sum, print];
    nodes.sort_unstable();
    let mut touching = vec![
        (g.input("sum", 0), g.output("get_a", 0)),
        (g.input("sum", 1), g.output("get_b", 0)),
        (g.input("mult", 0), g.output("sum", 0)),
        (g.input("Print", 0), g.output("mult", 0)),
    ];
    touching.sort_unstable();
    assert_eq!(
        wiring.touching(&nodes).wires().collect::<Vec<_>>(),
        touching
    );
    let mut rest = wiring.clone();
    assert_eq!(rest.remove_touching(&nodes), wiring.touching(&nodes));
    assert_eq!(
        rest.wires().collect::<Vec<_>>(),
        [(g.input("mult", 1), g.output("get_b", 0))]
    );

    let event = |node, number| EventPort::new(node, PortId::new(number));
    let mut subscribed = Wiring::default();
    let mut from_a = [
        Subscription::new(event(get_a, 1), mult),
        Subscription::new(event(get_a, 2), sum),
    ];
    from_a.sort_unstable();
    for subscription in from_a {
        assert!(subscribed.subscribe(subscription));
    }
    assert!(subscribed.subscribe(Subscription::new(event(get_b, 1), sum)));
    assert!(!subscribed.subscribe(from_a[0]), "made once");
    assert_eq!(
        subscribed.subscriptions_from(get_a).collect::<Vec<_>>(),
        from_a
    );
    assert!(subscribed.is_subscribed(from_a[1]));
    assert!(!subscribed.is_subscribed(Subscription::new(event(get_b, 2), sum)));

    let port = g.input("sum", 0);
    let pairs = Wiring::from([
        (port, Binding::Const(1i64.into())),
        (port, Binding::Const(2i64.into())),
    ]);
    assert_eq!(
        pairs.bindings().collect::<Vec<_>>(),
        [(port, &Binding::Const(2i64.into()))]
    );
}

/// The ghost wiring answers the two reverse questions the editor asks of a node, the ghost wires
/// that read it and the ghost subscriptions that land on it, and counts what touches it.
#[test]
fn ghost_wiring_answers_what_reads_a_node_and_lands_on_it() {
    let g = TestGraph::sample();
    let [get_a, get_b, sum, mult] = ["get_a", "get_b", "sum", "mult"].map(|name| g.id(name));
    let mut ghosts = GhostWiring::default();
    let wire = |from: &str| GhostBinding::from(Binding::Wire(g.output(from, 0)));
    ghosts.bindings.insert(g.input("sum", 0), wire("get_a"));
    ghosts.bindings.insert(g.input("sum", 1), wire("get_b"));
    ghosts.bindings.insert(g.input("mult", 1), wire("get_b"));
    ghosts.bindings.insert(
        g.input("mult", 0),
        GhostBinding::from(Binding::Const(3i64.into())),
    );
    let to_sum = Subscription::new(EventPort::new(get_a, PortId::new(1)), sum);
    let to_mult = Subscription::new(EventPort::new(get_b, PortId::new(1)), mult);
    for subscription in [to_sum, to_mult] {
        ghosts
            .subscriptions
            .insert(subscription, GhostSubscription::default());
    }

    let mut from_b = vec![g.input("sum", 1), g.input("mult", 1)];
    from_b.sort_unstable();
    assert_eq!(
        ghosts
            .wires_from(get_b)
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        from_b
    );
    assert_eq!(
        ghosts.wires_from(sum).count(),
        0,
        "a constant reads nothing"
    );
    assert_eq!(
        ghosts
            .subscriptions_to(sum)
            .map(|(subscription, _)| subscription)
            .collect::<Vec<_>>(),
        [to_sum]
    );
    // `get_b`: two wires and a subscription; `mult`: two bindings and a subscription.
    assert_eq!(ghosts.count_touching(get_b), 3);
    assert_eq!(ghosts.count_touching(mult), 3);

    let mut nodes = [get_a, mult];
    nodes.sort_unstable();
    let mut rest = ghosts.clone();
    let taken = rest.remove_touching(&nodes);
    assert_eq!(taken, ghosts.touching(&nodes));
    assert_eq!(
        rest.bindings().map(|(port, _)| port).collect::<Vec<_>>(),
        [g.input("sum", 1)]
    );
    assert!(rest.subscriptions().next().is_none());
}

/// A bind is admitted where both ends are nodes, the input takes it and it closes no loop; each
/// refusal names its own cause. What an input is wired to reads alike, live or a ghost.
#[test]
fn a_bind_is_held_to_its_nodes_its_input_and_the_loops_it_would_close() {
    let mut g = TestGraph::sample();
    let sum_in = g.input("sum", 0);
    let admit = |g: &TestGraph, binding: Binding| g.graph().admit_bind(g.scope(), sum_in, &binding);
    assert_eq!(admit(&g, Binding::Wire(g.output("get_b", 0))), Ok(()));
    assert_eq!(
        admit(&g, Binding::Wire(g.output("mult", 0))),
        Err(BindRejection::Cycle),
        "`sum` feeds `mult`"
    );
    assert_eq!(
        admit(&g, Binding::Const(ConstValue::String("x".to_owned()))),
        Err(BindRejection::Binding(BindingRejection::ConstMismatch))
    );

    assert_eq!(
        g.graph().input_state_wire(sum_in),
        Some(g.output("get_a", 0))
    );
    g.graph_mut()
        .set_input_binding(sum_in, Binding::Const(1i64.into()));
    assert_eq!(g.graph().input_state_wire(sum_in), None, "a constant");
    g.graph_mut().set_input_binding(sum_in, None);
    assert_eq!(g.graph().input_state_wire(sum_in), None, "nothing");

    // Onto a node an edit is inserting with its bindings, read off its declaration alone.
    let fresh = NodeId::unique();
    {
        let scope = g.scope();
        let decl = g.graph().find(g.id("sum")).unwrap().decl(scope);
        let onto = |port: u32, binding: &Binding| {
            g.graph().admit_onto(
                scope,
                decl,
                InputPort::new(fresh, PortId::new(port)),
                binding,
            )
        };
        let int = Binding::Const(1i64.into());
        assert_eq!(onto(1, &int), Ok(()));
        assert_eq!(onto(1, &Binding::Wire(g.output("get_a", 0))), Ok(()));
        assert_eq!(onto(9, &int), Err(BindingRejection::UndeclaredInput));
    }

    let get_b_out = g.output("get_b", 0);
    let get_b = ghost(&mut g, "get_b", DetachReason::TooLarge);
    assert_eq!(
        admit(&g, Binding::Wire(get_b_out)),
        Err(BindRejection::GhostNode { node_id: get_b })
    );
    assert_eq!(
        g.graph().input_state_wire(g.input("mult", 1)),
        Some(get_b_out),
        "a ghost wire reads alike"
    );
}

/// What a port shows: its node's declaration, else the port a ghost saved, else its ghost node's
/// saved declaration; nothing where none knows it. An event likewise, by name.
#[test]
fn a_port_shows_what_the_first_declaration_that_knows_it_gives() {
    let mut g = TestGraph::sample();
    let sum = g.id("sum");
    let declared = &g.graph().find(sum).unwrap().decl(g.scope()).inputs()[0];
    let (declared_name, declared_type) = (declared.name.clone(), declared.data_type);
    let saved = SavedPort {
        name: "kept".to_owned(),
        data_type: DataType::Float,
    };
    let lost = InputPort::new(sum, PortId::new(9));
    {
        let (graph, scope) = (g.graph(), g.scope());
        assert_eq!(
            graph.shown_input(scope, g.input("sum", 0), Some(&saved)),
            Some(ShownPort {
                name: &declared_name,
                data_type: declared_type,
            }),
            "the declaration outranks a saved port"
        );
        assert_eq!(
            graph.shown_input(scope, lost, Some(&saved)),
            Some(ShownPort::from(&saved))
        );
        assert_eq!(graph.shown_input(scope, lost, None), None);
        assert_eq!(
            graph.shown_output(scope, OutputPort::new(sum, PortId::new(9)), None),
            None
        );
        assert_eq!(
            graph.shown_event(scope, EventPort::new(sum, PortId::new(9)), None),
            None
        );
    }

    // A ghost node shows its saved declaration's ports and events.
    let get_a = g.id("get_a");
    g.graph_mut().detach_into_ghosts(
        vec![Detaching {
            node_id: get_a,
            reason: DetachReason::TooLarge,
            saved: Some(SavedDecl {
                outputs: vec![Numbered {
                    id: PortId::new(1),
                    port: saved.clone(),
                }],
                events: vec![Numbered {
                    id: PortId::new(2),
                    port: SavedEvent {
                        name: "fired".to_owned(),
                    },
                }],
                ..SavedDecl::default()
            }),
        }],
        &mut Vec::new(),
    );
    let (graph, scope) = (g.graph(), g.scope());
    assert_eq!(
        graph.shown_output(scope, OutputPort::new(get_a, PortId::new(1)), None),
        Some(ShownPort::from(&saved))
    );
    assert_eq!(
        graph.shown_event(scope, EventPort::new(get_a, PortId::new(2)), None),
        Some("fired")
    );
}
