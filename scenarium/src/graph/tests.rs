use crate::data::DataType;
use crate::graph::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
use crate::graph::{
    Binding, CachePersistence, Graph, InputPort, Node, NodeId, NodeKind, OutputPort, Subscription,
    closes_data_cycle,
};
use crate::node::function::{Func, FuncId, FuncInput, FuncOutput};
use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
use common::SerdeFormat;

#[test]
fn roundtrip_serialization() -> anyhow::Result<()> {
    let graph = test_graph();

    for format in SerdeFormat::all_formats_for_testing() {
        let serialized = graph.serialize(format)?;
        let deserialized = Graph::deserialize(&serialized, format)?;
        let serialized_again = deserialized.serialize(format)?;
        assert_eq!(serialized, serialized_again);
    }

    let bin = graph.serialize(SerdeFormat::Bitcode)?;
    let deserialized = Graph::deserialize(&bin, SerdeFormat::Bitcode)?;
    assert_eq!(graph, deserialized);

    Ok(())
}

#[test]
fn check_passes_for_valid_graph() {
    assert!(test_graph().check().is_ok());
}

#[test]
fn persist_round_trips_and_defaults_to_memory() {
    use common::deserialize;
    assert_eq!(CachePersistence::default(), CachePersistence::Memory);

    let library = test_func_lib(TestFuncHooks::default());
    let mut graph = Graph::default();
    let mut node: Node = library.by_name("get_a").unwrap().into();
    node.persist = CachePersistence::Disk;
    graph.add(node);

    for format in [SerdeFormat::Json, SerdeFormat::Bitcode] {
        let bytes = graph.serialize(format).unwrap();
        let back = Graph::deserialize(&bytes, format).unwrap();
        assert_eq!(
            back.by_name("get_a").unwrap().persist,
            CachePersistence::Disk
        );
    }

    // A node authored before `persist` existed deserializes as `Memory`.
    let legacy = r#"{ "id": "00000000-0000-0000-0000-000000000001",
            "kind": { "Func": "00000000-0000-0000-0000-000000000002" }, "name": "n" }"#;
    let node: Node = deserialize(legacy.as_bytes(), SerdeFormat::Json).unwrap();
    assert_eq!(node.persist, CachePersistence::Memory);
}

#[test]
fn check_rejects_dangling_binding() {
    let mut graph = test_graph();
    let sum_id = graph.by_name("sum").unwrap().id;
    // Repoint sum's input at a node that doesn't exist.
    graph.set_input_binding(InputPort::new(sum_id, 0), (NodeId::unique(), 0).into());

    let err = graph.check().expect_err("dangling binding must fail check");
    assert!(err.to_string().contains("binds to missing node"));
}

#[test]
fn const_only_input_rejects_bind_but_a_normal_input_accepts_it() {
    use crate::library::Library;
    use crate::node::function::FuncId;

    // One Int-in / Int-out func, so a wire between two instances is otherwise
    // valid — only the `const_only` flag decides whether check accepts it.
    let check = |const_only: bool| -> anyhow::Result<()> {
        let port = FuncInput::required("locked", DataType::Int);
        let port = if const_only { port.const_only() } else { port };
        let func = Func::new(FuncId::unique(), "f")
            .input(port)
            .output(FuncOutput::new("out", DataType::Int));
        let mut library = Library::default();
        library.funcs.add(func.clone());

        let mut graph = Graph::default();
        let producer = graph.add_func_node(&func);
        let consumer = graph.add_func_node(&func);
        graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
        graph.check_with(&library)
    };

    assert!(
        check(false).is_ok(),
        "a normal input accepts a wired binding"
    );
    let err = check(true).expect_err("a const-only input must reject a wired binding");
    assert!(
        err.to_string().contains("const-only"),
        "unexpected error: {err}"
    );
}

#[test]
fn check_with_rejects_type_mismatched_bindings_through_passthroughs() {
    use crate::data::{DataType, StaticValue};
    use crate::library::Library;
    use crate::node::special::SpecialNode;

    // Int and String never coerce (numerics coerce among themselves, but a
    // string is a distinct kind), so this pair exercises a real rejection.
    let int_src =
        Func::new(FuncId::unique(), "int_src").output(FuncOutput::new("o", DataType::Int));
    let str_sink = Func::new(FuncId::unique(), "str_sink")
        .input(FuncInput::required("x", DataType::String))
        .output(FuncOutput::new("o", DataType::String));
    let int_sink = Func::new(FuncId::unique(), "int_sink")
        .input(FuncInput::required("x", DataType::Int))
        .output(FuncOutput::new("o", DataType::Int));
    let mut library = Library::default();
    library.funcs.add(int_src.clone());
    library.funcs.add(str_sink.clone());
    library.funcs.add(int_sink.clone());

    let pass = || {
        Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
            bypass: false,
        }))
    };

    // Direct Int → String is rejected at the compile boundary.
    let mut g = Graph::default();
    let s = g.add_func_node(&int_src);
    let f = g.add_func_node(&str_sink);
    g.set_input_binding(InputPort::new(f, 0), Binding::bind(s, 0));
    let err = g
        .check_with(&library)
        .expect_err("Int into a String input must be rejected");
    assert!(
        err.to_string().contains("incompatible"),
        "unexpected: {err}"
    );

    // Direct Int → Int is accepted.
    let mut g = Graph::default();
    let s = g.add_func_node(&int_src);
    let i = g.add_func_node(&int_sink);
    g.set_input_binding(InputPort::new(i, 0), Binding::bind(s, 0));
    assert!(g.check_with(&library).is_ok());

    // The check resolves *through* a passthrough: Int → pass → Int is fine,
    // Int → pass → String is rejected (the wildcard carries the real type).
    let mut g = Graph::default();
    let s = g.add_func_node(&int_src);
    let p = pass();
    let pid = p.id;
    g.add(p);
    g.set_input_binding(InputPort::new(pid, 0), Binding::bind(s, 0));
    let i = g.add_func_node(&int_sink);
    g.set_input_binding(InputPort::new(i, 0), Binding::bind(pid, 0));
    assert!(
        g.check_with(&library).is_ok(),
        "Int through a passthrough into Int is compatible"
    );

    g.set_input_binding(InputPort::new(i, 0), Binding::None);
    let f = g.add_func_node(&str_sink);
    g.set_input_binding(InputPort::new(f, 0), Binding::bind(pid, 0));
    assert!(
        g.check_with(&library)
            .is_err_and(|e| e.to_string().contains("incompatible")),
        "Int through a passthrough into String must be rejected"
    );

    // Constants are type-checked too: a String literal can't satisfy an Int
    // input, but a numeric literal can (the scalar coercion).
    let mut g = Graph::default();
    let i = g.add_func_node(&int_sink);
    g.set_input_binding(
        InputPort::new(i, 0),
        Binding::Const(StaticValue::String("x".into())),
    );
    assert!(
        g.check_with(&library)
            .is_err_and(|e| e.to_string().contains("incompatible")),
        "a String constant on an Int input must be rejected"
    );
    g.set_input_binding(
        InputPort::new(i, 0),
        Binding::Const(StaticValue::Float(2.5)),
    );
    assert!(
        g.check_with(&library).is_ok(),
        "a numeric constant satisfies a numeric input"
    );
}

#[test]
fn resolve_output_type_follows_passthrough_chain() {
    use crate::data::{DataType, StaticValue};
    use crate::library::Library;
    use crate::node::special::SpecialNode;

    // Int-out producer → pass1 → pass2. Both passthroughs declare a `Null`
    // (wildcard) output, but the resolved type must be the producer's `Int`.
    let producer = Func::new(FuncId::unique(), "src").output(FuncOutput::new("out", DataType::Int));
    let mut library = Library::default();
    library.funcs.add(producer.clone());

    let mut graph = Graph::default();
    let src = graph.add_func_node(&producer);
    let pass1 = Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
        bypass: false,
    }));
    let pass2 = Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
        bypass: false,
    }));
    let (p1, p2) = (pass1.id, pass2.id);
    graph.add(pass1);
    graph.add(pass2);
    graph.set_input_binding(InputPort::new(p1, 0), Binding::bind(src, 0));
    graph.set_input_binding(InputPort::new(p2, 0), Binding::bind(p1, 0));

    // The producer reports its own declared type.
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(src, 0)),
        DataType::Int
    );
    // Each passthrough mirrors what flows through, transitively.
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p1, 0)),
        DataType::Int
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p2, 0)),
        DataType::Int
    );

    // An unbound value input leaves the passthrough polymorphic (`Null`),
    // so its output accepts any consumer again.
    graph.set_input_binding(InputPort::new(p1, 0), Binding::None);
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p1, 0)),
        DataType::Null
    );
    // The taint flows downstream: pass2 now reads pass1's `Null`.
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p2, 0)),
        DataType::Null
    );

    // A scalar const carries its type, so the output resolves to it (and
    // propagates downstream) — a const isn't "no type".
    graph.set_input_binding(
        InputPort::new(p1, 0),
        Binding::Const(StaticValue::Bool(true)),
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p1, 0)),
        DataType::Bool
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p2, 0)),
        DataType::Bool,
        "the const's type propagates through the second passthrough too"
    );

    // A const whose type can't be reconstructed from the value alone — an
    // enum literal on a `Null` (wildcard) input — stays polymorphic rather
    // than panicking. (The passthrough's value input is `Null`-declared.)
    graph.set_input_binding(
        InputPort::new(p1, 0),
        Binding::Const(StaticValue::Enum("X".into())),
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p1, 0)),
        DataType::Null
    );
}

#[test]
fn resolve_output_type_uses_declared_type_for_typed_const_input() {
    use crate::data::{DataType, FsPathConfig, FsPathMode, StaticValue, TypeId};
    use crate::library::Library;
    use std::sync::Arc;

    // A reroute func with *typed* inputs, each mirrored by a wildcard output.
    let fs_ty = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)));
    let enum_ty = DataType::Enum(TypeId::from_u128(0x5e));
    let func = Func::new(FuncId::unique(), "reroute")
        .input(FuncInput::required("path", fs_ty.clone()))
        .input(FuncInput::required("mode", enum_ty.clone()))
        .wildcard_output("path_out", 0)
        .wildcard_output("mode_out", 1);
    let mut library = Library::default();
    library.funcs.add(func.clone());

    let mut graph = Graph::default();
    let n = graph.add_func_node(&func);

    // A const FsPath / Enum on a typed input resolves to that input's
    // *declared* type — which carries the full `FsPathConfig` / `Enum` id the
    // bare `StaticValue` lacks (this is the case that used to be unimplemented).
    graph.set_input_binding(
        InputPort::new(n, 0),
        Binding::Const(StaticValue::FsPath("/tmp/x".into())),
    );
    graph.set_input_binding(
        InputPort::new(n, 1),
        Binding::Const(StaticValue::Enum("A".into())),
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(n, 0)),
        fs_ty
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(n, 1)),
        enum_ty
    );
}

#[test]
fn edges_invalidated_by_follows_wildcard_chains() {
    use crate::data::DataType;
    use crate::library::Library;
    use crate::node::special::SpecialNode;

    let float_src =
        Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float));
    let str_src =
        Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String));
    let float_sink = Func::new(FuncId::unique(), "fsink")
        .input(FuncInput::required("x", DataType::Float))
        .output(FuncOutput::new("o", DataType::Float));
    let mut library = Library::default();
    library.funcs.add(float_src.clone());
    library.funcs.add(str_src.clone());
    library.funcs.add(float_sink.clone());

    let add_pass = |g: &mut Graph| {
        let node = Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
            bypass: false,
        }));
        let id = node.id;
        g.add(node);
        id
    };

    // Float producer → pass1 → pass2 → Float sink: a valid chain.
    let mut g = Graph::default();
    let fp = g.add_func_node(&float_src);
    let sp = g.add_func_node(&str_src);
    let p1 = add_pass(&mut g);
    let p2 = add_pass(&mut g);
    let sink = g.add_func_node(&float_sink);
    g.set_input_binding(InputPort::new(p1, 0), Binding::bind(fp, 0));
    g.set_input_binding(InputPort::new(p2, 0), Binding::bind(p1, 0));
    g.set_input_binding(InputPort::new(sink, 0), Binding::bind(p2, 0));

    // Rewire pass1's value input to the String producer: pass1.out and
    // pass2.out both retype to String, so the *two-hops-down* sink edge is
    // the one now incompatible — the chain must be followed to find it.
    g.set_input_binding(InputPort::new(p1, 0), Binding::bind(sp, 0));
    assert_eq!(
        g.edges_invalidated_by(&library, p1, 0),
        vec![InputPort::new(sink, 0)],
        "the edge two passthroughs downstream is flagged"
    );

    // Changing an ordinary node's input retypes nothing → no invalidations.
    assert!(g.edges_invalidated_by(&library, sink, 0).is_empty());
    // Changing the passthrough's *path* input (no output mirrors it) → none.
    assert!(g.edges_invalidated_by(&library, p1, 1).is_empty());
}

#[test]
fn would_create_cycle_detects_direct_and_transitive_loops() {
    // A relay func (one input, one output) lets a node be both producer and
    // consumer. Chain a → b → c via binds; d is an unconnected sink.
    let relay = Func::new(FuncId::unique(), "relay")
        .input(FuncInput::required("x", DataType::Int))
        .output(FuncOutput::new("o", DataType::Int));
    let mut g = Graph::default();
    let a = g.add_func_node(&relay);
    let b = g.add_func_node(&relay);
    let c = g.add_func_node(&relay);
    let d = g.add_func_node(&relay);
    g.set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    g.set_input_binding(InputPort::new(c, 0), Binding::bind(b, 0));

    // Back-edges close a loop: the producer is reachable from the consumer.
    assert!(g.would_create_cycle(b, a), "b → a closes a → b");
    assert!(
        g.would_create_cycle(c, a),
        "c → a closes a → b → c transitively"
    );
    // A node wired to itself is a self-cycle.
    assert!(g.would_create_cycle(a, a));

    // Forward / sideways edges are fine — a second a → c path is a DAG
    // diamond, and a fresh sink is reachable from nothing.
    assert!(!g.would_create_cycle(a, c));
    assert!(!g.would_create_cycle(c, d));
    assert!(!g.would_create_cycle(a, d));

    // The free core matches the wrapper on a raw `(producer, consumer)` edge
    // list — the path the editor's scene-based pre-filter takes.
    let edges = [(a, b), (b, c)];
    assert!(closes_data_cycle(edges.into_iter(), c, a));
    assert!(!closes_data_cycle(edges.into_iter(), a, c));
    assert!(closes_data_cycle(edges.into_iter(), a, a));
}

#[test]
fn resolve_output_type_breaks_a_binding_cycle() {
    use crate::data::DataType;
    use crate::library::Library;
    use crate::node::special::SpecialNode;

    // A passthrough whose value input binds to its own output — a cycle the
    // editor can momentarily hold. Resolution must terminate as `Null`.
    let library = Library::default();
    let mut graph = Graph::default();
    let pass = Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
        bypass: false,
    }));
    let id = pass.id;
    graph.add(pass);
    graph.set_input_binding(InputPort::new(id, 0), Binding::bind(id, 0));

    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(id, 0)),
        DataType::Null
    );
}

#[test]
fn input_type_resolves_declared_types_and_skips_boundaries() {
    use crate::data::DataType;
    use crate::library::Library;

    let consumer = Func::new(FuncId::unique(), "dst")
        .input(FuncInput::required("x", DataType::Float))
        .output(FuncOutput::new("out", DataType::Float));
    let mut library = Library::default();
    library.funcs.add(consumer.clone());

    let mut graph = Graph::default();
    let dst = graph.add_func_node(&consumer);
    assert_eq!(
        graph.input_type(&library, InputPort::new(dst, 0)),
        Some(DataType::Float)
    );
    // Out-of-range port → None.
    assert_eq!(graph.input_type(&library, InputPort::new(dst, 9)), None);

    // A boundary node carries no per-port type here → None (caller's Null).
    let boundary = Node::new(NodeKind::SubgraphInput);
    let b = boundary.id;
    graph.add(boundary);
    assert_eq!(graph.input_type(&library, InputPort::new(b, 0)), None);
}

#[test]
fn deserialize_rejects_corrupt_graph() {
    let mut graph = test_graph();
    let sum_id = graph.by_name("sum").unwrap().id;
    graph.set_input_binding(InputPort::new(sum_id, 0), (NodeId::unique(), 0).into());

    // serialize doesn't validate; deserialize must reject the dangling bind
    // (the release-path structural guard, not a debug-only assert).
    let bytes = graph.serialize(SerdeFormat::Bitcode).unwrap();
    assert!(Graph::deserialize(&bytes, SerdeFormat::Bitcode).is_err());
}

#[test]
fn node_remove_test() -> anyhow::Result<()> {
    let mut graph = test_graph();

    let node_id = graph.by_name("sum").unwrap().id;
    graph.remove_by_id(node_id);

    assert!(graph.by_name("sum").is_none());
    assert_eq!(graph.len(), 4);

    // No surviving edge references the removed node (as consumer or producer).
    for (dst, src) in graph.edges() {
        assert_ne!(dst.node_id, node_id);
        assert_ne!(src.node_id, node_id);
    }

    Ok(())
}

// === Accessors ===

#[test]
fn node_kind_accessors() {
    let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
    let func = NodeKind::Func(func_id);
    assert_eq!(func.as_func(), Some(func_id));
    assert_eq!(func.as_subgraph(), None);
    assert!(!func.is_boundary());

    let sub_id = SubgraphId::unique();
    let sub = NodeKind::Subgraph(SubgraphRef::Local(sub_id));
    assert_eq!(sub.as_func(), None);
    assert_eq!(sub.as_subgraph().map(|r| r.id()), Some(sub_id));
    assert!(!sub.is_boundary());

    assert!(NodeKind::SubgraphInput.is_boundary());
    assert!(NodeKind::SubgraphOutput.is_boundary());
    assert_eq!(NodeKind::SubgraphInput.as_func(), None);
    assert_eq!(NodeKind::SubgraphOutput.as_subgraph(), None);
}

#[test]
fn node_func_id_shims_kind() {
    let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
    assert_eq!(Node::new(NodeKind::Func(func_id)).func_id(), Some(func_id));
    assert_eq!(Node::new(NodeKind::SubgraphInput).func_id(), None);
}

#[test]
fn binding_accessors() {
    let out = OutputPort::new(NodeId::unique(), 2);
    let bind = Binding::Bind(out);
    assert_eq!(bind.as_output_binding(), Some(&out));
    assert!(bind.is_some());
    assert!(!bind.is_none());

    let konst = Binding::from(5i64);
    assert_eq!(konst.as_output_binding(), None);
    assert!(konst.is_some()); // a Const is a real binding
    assert!(!konst.is_none());

    let none = Binding::None;
    assert_eq!(none.as_output_binding(), None);
    assert!(!none.is_some());
    assert!(none.is_none());
}

#[test]
fn binding_conversions() {
    let nid = NodeId::unique();
    let from_port: Binding = OutputPort::new(nid, 1).into();
    let from_tuple: Binding = (nid, 1usize).into();
    assert_eq!(from_port, from_tuple);
    assert_eq!(from_port.as_output_binding().unwrap().port_idx, 1);

    assert_eq!(Binding::from(7i64), Binding::Const(7i64.into()));
}

// === Data bindings ===

#[test]
fn node_bindings_yields_ports_in_order_with_none_gaps() {
    let graph = test_graph();
    let sum_id = graph.by_name("sum").unwrap().id;
    let get_a_id = graph.by_name("get_a").unwrap().id;
    let get_b_id = graph.by_name("get_b").unwrap().id;

    // sum has two bound inputs; ask for arity 3 to exercise the unbound gap.
    let bindings: Vec<_> = graph.node_bindings(sum_id, 3).collect();
    assert_eq!(
        bindings,
        vec![
            (0, Binding::bind(get_a_id, 0)),
            (1, Binding::bind(get_b_id, 0)),
            (2, Binding::None),
        ]
    );
}

// === Event subscriptions ===

#[test]
fn subscribe_unsubscribe_is_subscribed() {
    let graph = test_graph();
    let emitter = graph.by_name("get_a").unwrap().id;
    let sub = graph.by_name("sum").unwrap().id;
    let mut graph = graph;

    assert!(!graph.is_subscribed(emitter, 0, sub));
    graph.subscribe(emitter, 0, sub);
    assert!(graph.is_subscribed(emitter, 0, sub));

    // Distinct event_idx is a distinct edge.
    assert!(!graph.is_subscribed(emitter, 1, sub));

    // Re-subscribing is idempotent (BTreeSet dedups).
    graph.subscribe(emitter, 0, sub);
    assert_eq!(graph.subscriptions().count(), 1);

    graph.unsubscribe(emitter, 0, sub);
    assert!(!graph.is_subscribed(emitter, 0, sub));
    assert_eq!(graph.subscriptions().count(), 0);
}

#[test]
fn subscribers_ranges_one_emitter_event() {
    let mut graph = test_graph();
    let emitter = graph.by_name("get_a").unwrap().id;
    let s1 = graph.by_name("sum").unwrap().id;
    let s2 = graph.by_name("mult").unwrap().id;
    let other = graph.by_name("Print").unwrap().id;

    graph.subscribe(emitter, 0, s1);
    graph.subscribe(emitter, 0, s2);
    graph.subscribe(emitter, 1, other); // different event: must not leak in

    let mut got: Vec<NodeId> = graph.subscribers(emitter, 0).collect();
    got.sort();
    let mut want = vec![s1, s2];
    want.sort();
    assert_eq!(got, want);

    assert_eq!(
        graph.subscribers(emitter, 1).collect::<Vec<_>>(),
        vec![other]
    );
    assert_eq!(graph.subscribers(emitter, 2).count(), 0);
}

// === Snapshot / restore (editor undo) ===

#[test]
fn wiring_snapshot_round_trips_through_restore() {
    let mut graph = test_graph();
    let sum_id = graph.by_name("sum").unwrap().id;
    let get_a_id = graph.by_name("get_a").unwrap().id;

    // Add a subscription that touches `sum` so both arms are exercised.
    graph.subscribe(get_a_id, 0, sum_id);

    let node = graph.by_id(&sum_id).unwrap().clone();
    let bindings = graph.bindings_touching(sum_id);
    let subs = graph.subscriptions_touching(sum_id);

    // sum touches: its own inputs (sum,0),(sum,1) + the edge (mult,0)<-sum.
    assert_eq!(bindings.len(), 3);
    assert_eq!(
        subs,
        vec![Subscription {
            emitter: get_a_id,
            event_idx: 0,
            subscriber: sum_id
        }]
    );

    let edges_before = graph.edges().count();
    graph.remove_by_id(sum_id);
    assert_eq!(graph.edges().count(), edges_before - 3);
    assert!(graph.subscriptions_touching(sum_id).is_empty());

    // Undo: re-add the node, then re-apply its wiring.
    graph.add(node);
    graph.restore_wiring(&bindings, &subs);

    assert_eq!(graph.edges().count(), edges_before);
    assert!(graph.is_subscribed(get_a_id, 0, sum_id));
    assert_eq!(graph.bindings_touching(sum_id), bindings);
}

// === Construction helpers seed default const bindings ===

fn func_with_default(default: i64) -> Func {
    Func::new(FuncId::unique(), "withdefault")
        .input(FuncInput::optional("x", DataType::Int).default(default))
}

#[test]
fn add_func_node_seeds_default_const_binding() {
    let func = func_with_default(7);
    let mut graph = Graph::default();
    let id = graph.add_func_node(&func);

    assert_eq!(graph.by_id(&id).unwrap().func_id(), Some(func.id));
    assert_eq!(
        graph.input_binding(InputPort::new(id, 0)),
        Binding::Const(7i64.into())
    );
}

#[test]
fn add_func_node_leaves_defaultless_inputs_unbound() {
    let library = test_func_lib(TestFuncHooks::default());
    let sum = library.by_name("sum").unwrap(); // inputs have no defaults
    let mut graph = Graph::default();
    let id = graph.add_func_node(sum);

    assert_eq!(graph.input_binding(InputPort::new(id, 0)), Binding::None);
    assert_eq!(graph.input_binding(InputPort::new(id, 1)), Binding::None);
}

#[test]
fn add_subgraph_node_seeds_default_const_binding() {
    let mut input = FuncInput::optional("A", DataType::Int).default(3i64);
    let def = SubgraphDef::new(SubgraphId::unique(), "Def")
        .category("Test")
        .inputs([input.clone(), {
            input.default_value = None;
            input
        }]);

    let mut graph = Graph::default();
    let id = graph.add_subgraph_node(&def, SubgraphRef::Local(def.id));

    // Port 0 had a default; port 1 did not.
    assert_eq!(
        graph.input_binding(InputPort::new(id, 0)),
        Binding::Const(3i64.into())
    );
    assert_eq!(graph.input_binding(InputPort::new(id, 1)), Binding::None);
}

// === Definition resolution ===

#[test]
fn resolve_def_picks_local_or_linked_source() {
    let mut library = test_func_lib(TestFuncHooks::default());

    let linked_id = SubgraphId::unique();
    library.add_subgraph(SubgraphDef::new(linked_id, "Linked").category("Test"));

    let mut graph = Graph::default();
    let local_id = SubgraphId::unique();
    graph
        .subgraphs
        .add(SubgraphDef::new(local_id, "Local").category("Test"));

    assert_eq!(
        graph
            .resolve_def(SubgraphRef::Local(local_id), &library)
            .unwrap()
            .name,
        "Local"
    );
    assert_eq!(
        graph
            .resolve_def(SubgraphRef::Linked(linked_id), &library)
            .unwrap()
            .name,
        "Linked"
    );
    // A local ref whose id only exists in the library does not resolve.
    assert!(
        graph
            .resolve_def(SubgraphRef::Local(linked_id), &library)
            .is_none()
    );
}

#[test]
fn prune_subscriptions_drops_out_of_range_and_missing_emitter() {
    use crate::library::Library;
    use crate::node::event_lambda::EventLambda;

    // A func exposing exactly one event: idx 0 is valid, idx 1+ are not.
    let func_id = FuncId::from_u128(0xe0e0);
    let mut library = Library::default();
    library.add(Func::new(func_id, "emitter").event("tick", EventLambda::default()));

    let mut graph = Graph::default();
    let emitter = Node::new(NodeKind::Func(func_id));
    let emitter_id = emitter.id;
    let subscriber = Node::new(NodeKind::Func(func_id));
    let subscriber_id = subscriber.id;
    graph.add(emitter);
    graph.add(subscriber);

    // A real but absent emitter id models one whose node was removed.
    let ghost = NodeId::unique();
    graph.subscribe(emitter_id, 0, subscriber_id); // valid
    graph.subscribe(emitter_id, 1, subscriber_id); // event_idx past the one event
    graph.subscribe(ghost, 0, subscriber_id); // emitter doesn't exist

    let removed = graph.prune_dangling_wiring(&library);
    assert_eq!(removed, 2, "out-of-range and missing-emitter edges drop");
    assert!(
        graph.is_subscribed(emitter_id, 0, subscriber_id),
        "the in-range edge survives"
    );
    assert!(!graph.is_subscribed(emitter_id, 1, subscriber_id));
    assert!(!graph.is_subscribed(ghost, 0, subscriber_id));

    // Idempotent on an already-valid graph.
    assert_eq!(graph.prune_dangling_wiring(&library), 0);

    // An emitter whose func is missing from the library has unknowable
    // arity, so its subscription is kept (not panicked on, not dropped).
    let ghost = Node::new(NodeKind::Func(FuncId::from_u128(0xdead)));
    let ghost_id = ghost.id;
    graph.add(ghost);
    graph.subscribe(ghost_id, 4, subscriber_id);
    assert_eq!(graph.prune_dangling_wiring(&library), 0);
    assert!(graph.is_subscribed(ghost_id, 4, subscriber_id));
}

#[test]
fn prune_bindings_drops_out_of_range_and_missing_endpoints() {
    use crate::library::Library;

    // Func with one input + one output, so the only in-range port idx is 0.
    let func_id = FuncId::from_u128(0xb12d);
    let mut library = Library::default();
    library.add(
        Func::new(func_id, "op")
            .input(FuncInput::optional("in", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int)),
    );

    let mut graph = Graph::default();
    let ids: Vec<NodeId> = (0..5)
        .map(|_| {
            let n = Node::new(NodeKind::Func(func_id));
            let id = n.id;
            graph.add(n);
            id
        })
        .collect();
    let (a, b, c, d, e) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let ghost = NodeId::unique();

    graph.set_input_binding(InputPort::new(b, 0), (a, 0).into()); // fully valid
    graph.set_input_binding(InputPort::new(c, 5), (a, 0).into()); // consumer input out of range
    graph.set_input_binding(InputPort::new(d, 0), (a, 9).into()); // producer output out of range
    graph.set_input_binding(InputPort::new(e, 0), (ghost, 0).into()); // producer node gone
    graph.set_input_binding(InputPort::new(ghost, 0), (a, 0).into()); // consumer node gone

    let removed = graph.prune_dangling_wiring(&library);
    assert_eq!(
        removed, 4,
        "every dangling binding drops, the valid one stays"
    );
    assert!(matches!(
        graph.input_binding(InputPort::new(b, 0)),
        Binding::Bind(_)
    ));
    for dead in [
        InputPort::new(c, 5),
        InputPort::new(d, 0),
        InputPort::new(e, 0),
        InputPort::new(ghost, 0),
    ] {
        assert!(matches!(graph.input_binding(dead), Binding::None));
    }

    // Const bindings are never structurally dangling — kept regardless.
    graph.set_input_binding(
        InputPort::new(b, 0),
        Binding::Const(crate::data::StaticValue::from(1i64)),
    );
    assert_eq!(graph.prune_dangling_wiring(&library), 0);

    // A node whose func is absent from the library (a stub for a doc saved
    // against a richer library) has unknowable arity, so its wiring is
    // kept — never panicked on, never dropped.
    let ghost_func = Node::new(NodeKind::Func(FuncId::from_u128(0xdead)));
    let ghost_id = ghost_func.id;
    graph.add(ghost_func);
    graph.set_input_binding(InputPort::new(ghost_id, 3), (a, 0).into());
    graph.set_input_binding(InputPort::new(b, 0), (ghost_id, 7).into());
    assert_eq!(
        graph.prune_dangling_wiring(&library),
        0,
        "unresolvable-node wiring is preserved"
    );
    assert!(matches!(
        graph.input_binding(InputPort::new(ghost_id, 3)),
        Binding::Bind(_)
    ));
}
