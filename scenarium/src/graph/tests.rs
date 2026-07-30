use crate::EventLambda;
use crate::graph::Graph;
use crate::graph::error::{GraphDeserializeError, GraphValidationError};
use crate::graph::func::{Func, FuncInput, FuncOutput};
use crate::graph::identity::FuncId;
use crate::graph::node::{CacheMode, Node, NodeKind};
use crate::graph::output_types::OutputTypes;
use crate::graph::{Binding, InputPort, NodeId, OutputPort};
use crate::library::Library;
use crate::testing::{self, TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, DetachedNode, StaticValue};
use ::common::{SerdeFormat, deserialize, serialize};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The effective type at one output port, through a table covering just this
/// graph — the shape a host uses, narrowed to a single question.
fn output_type(graph: &Graph, library: &Library, port: OutputPort) -> DataType {
    let mut types = OutputTypes::default();
    types.update(graph, library);
    types.get(port).cloned().unwrap_or_default()
}

/// A passthrough func — one `Any` input, one wildcard output mirroring it. The
/// generic hop for testing wildcard type resolution through a node.
fn passthrough_func() -> Func {
    testing::with_stub_lambda(
        Func::new(FuncId::unique(), "pass")
            .input(FuncInput::required("x", DataType::Any))
            .wildcard_output("o", 0),
    )
}

#[test]
fn validate_passes_for_valid_graph() {
    assert!(test_graph().validate().is_ok());
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
    // Distinct modes must not share a bit pattern (guards a botched refactor).
    assert_ne!(CacheMode::Ram, CacheMode::Disk);
    assert_ne!(CacheMode::None, CacheMode::Both);
}

#[test]
fn cache_mode_round_trips() {
    assert_eq!(CacheMode::default(), CacheMode::None);

    let library = test_func_lib(TestFuncHooks::default());
    for mode in [
        CacheMode::None,
        CacheMode::Ram,
        CacheMode::Disk,
        CacheMode::Both,
    ] {
        let mut graph = Graph::default();
        let mut node: Node = library.by_name("get_a").unwrap().into();
        node.cache = mode;
        graph.add(node);

        for format in [SerdeFormat::Json, SerdeFormat::Bitcode] {
            let bytes = graph.serialize(format).unwrap();
            let back = Graph::deserialize(&bytes, format).unwrap();
            assert_eq!(
                back.find_by_name("get_a").unwrap().cache,
                mode,
                "{mode:?} via {format:?}"
            );
        }
    }
}

#[test]
fn new_func_node_copies_its_func_default_cache_mode() {
    // A fresh func node inherits its func's `default_cache_mode` — the out-of-box
    // `None`, or whatever the func's builder raised it to.
    let plain = Func::new(FuncId::unique(), "plain");
    assert_eq!(
        Node::from(&plain).cache,
        CacheMode::None,
        "default func → no caching"
    );

    let hot = Func::new(FuncId::unique(), "hot").default_cache_mode(CacheMode::Both);
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

    // The func-less constructors have no func to copy from and seed `None`.
    assert_eq!(
        Node::new(NodeKind::Func(FuncId::unique())).cache,
        CacheMode::None
    );
}

#[test]
fn validate_rejects_dangling_binding() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum").unwrap().id;
    // Repoint sum's input at a node that doesn't exist.
    graph.set_input_binding(
        InputPort::new(sum_id, 0),
        Binding::bind(NodeId::unique(), 0),
    );

    let err = graph
        .validate()
        .expect_err("dangling binding must fail validation");
    assert!(err.to_string().contains("binds to missing node"));
}

#[test]
fn const_only_input_rejects_bind_but_a_normal_input_accepts_it() {
    use crate::graph::identity::FuncId;
    use crate::library::Library;

    // One Int-in / Int-out func, so a wire between two instances is otherwise
    // valid — only the `const_only` flag decides whether validation accepts it.
    let validate = |const_only: bool| -> Result<(), GraphValidationError> {
        let port = FuncInput::required("locked", DataType::Int);
        let port = if const_only { port.const_only() } else { port };
        let func = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "f")
                .input(port)
                .output(FuncOutput::new("out", DataType::Int)),
        );
        let mut library = Library::default();
        library.add(func.clone());

        let mut graph = Graph::default();
        let producer = graph.add_func_node(&func);
        let consumer = graph.add_func_node(&func);
        graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
        graph.validate_with(&library)
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
fn type_mismatches_degrade_at_flatten_not_at_validation() {
    use crate::execution::compile::Compiler;
    use crate::execution::compiled::ExecutionBinding;
    use crate::execution::identity::ExecutionNodeId;
    use crate::library::Library;
    use crate::{FsPathConfig, FsPathMode};
    use std::sync::Arc;

    // Int and String never coerce (numerics coerce among themselves, but a
    // string is a distinct kind), so this pair exercises a real mismatch.
    let int_src = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "int_src").output(FuncOutput::new("o", DataType::Int)),
    );
    let str_sink = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "str_sink")
            .input(FuncInput::required("x", DataType::String))
            .output(FuncOutput::new("o", DataType::String)),
    );
    let int_sink = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "int_sink")
            .input(FuncInput::required("x", DataType::Int))
            .output(FuncOutput::new("o", DataType::Int)),
    );
    let single_path = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "single_path")
            .input(FuncInput::required(
                "path",
                DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile))),
            ))
            .output(FuncOutput::new("o", DataType::Int)),
    );
    let path_list = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "path_list")
            .input(FuncInput::required(
                "paths",
                DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles))),
            ))
            .output(FuncOutput::new("o", DataType::Int)),
    );
    let mut library = Library::default();
    library.add(int_src.clone());
    library.add(str_sink.clone());
    library.add(int_sink.clone());
    library.add(single_path.clone());
    library.add(path_list.clone());

    // Validation always accepts; the compiled program's flat input shows
    // whether the binding survived the type gate or degraded to unbound.
    let flat_input = |g: &Graph, node: NodeId| {
        assert!(g.validate_with(&library).is_ok());
        let compiled = Compiler::default().compile(g, &library).unwrap();
        let e_node = &compiled[compiled.e_node_index[&ExecutionNodeId::from_node(node)]];
        compiled.inputs[e_node.inputs.nth(0)].binding.clone()
    };

    // Wires: Int -> String degrades, Int -> Int binds.
    let mut g = Graph::default();
    let s = g.add_func_node(&int_src);
    let f = g.add_func_node(&str_sink);
    let i = g.add_func_node(&int_sink);
    g.set_input_binding(InputPort::new(f, 0), Binding::bind(s, 0));
    g.set_input_binding(InputPort::new(i, 0), Binding::bind(s, 0));
    assert!(
        matches!(flat_input(&g, f), ExecutionBinding::None),
        "Int into a String input flattens as unbound"
    );
    assert!(
        matches!(flat_input(&g, i), ExecutionBinding::Bind(_)),
        "Int into an Int input binds"
    );

    // Constants: a String literal can't satisfy an Int input, a numeric one
    // can (scalar coercion), and the two FsPath shapes only satisfy their
    // matching picker mode.
    let cases = [
        (&int_sink, StaticValue::String("x".into()), false),
        (&int_sink, StaticValue::Float(2.5), true),
        (
            &single_path,
            StaticValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]),
            false,
        ),
        (&path_list, StaticValue::FsPath("a.fit".into()), false),
        (
            &path_list,
            StaticValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]),
            true,
        ),
    ];
    for (func, value, satisfied) in cases {
        let mut g = Graph::default();
        let node = g.add_func_node(func);
        g.set_input_binding(InputPort::new(node, 0), Binding::Const(value.clone()));
        assert_eq!(
            matches!(flat_input(&g, node), ExecutionBinding::Const(_)),
            satisfied,
            "const {value:?} on {:?}",
            func.name
        );
    }
}

#[test]
fn resolve_output_type_follows_passthrough_chain() {
    use crate::library::Library;
    use crate::{DataType, StaticValue};

    // Int-out producer → pass1 → pass2. Both passthroughs declare a `Any`
    // (wildcard) output, but the resolved type must be the producer's `Int`.
    let producer = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "src").output(FuncOutput::new("out", DataType::Int)),
    );
    let pass_func = passthrough_func();
    let mut library = Library::default();
    library.add(producer.clone());
    library.add(pass_func.clone());

    let mut graph = Graph::default();
    let src = graph.add_func_node(&producer);
    let p1 = graph.add_func_node(&pass_func);
    let p2 = graph.add_func_node(&pass_func);
    graph.set_input_binding(InputPort::new(p1, 0), Binding::bind(src, 0));
    graph.set_input_binding(InputPort::new(p2, 0), Binding::bind(p1, 0));

    // The producer reports its own declared type.
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(src, 0)),
        DataType::Int
    );
    // Each passthrough mirrors what flows through, transitively.
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p1, 0)),
        DataType::Int
    );
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p2, 0)),
        DataType::Int
    );

    // An unbound value input leaves the passthrough polymorphic (`Any`),
    // so its output accepts any consumer again.
    graph.set_input_binding(InputPort::new(p1, 0), None);
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p1, 0)),
        DataType::Any
    );
    // The taint flows downstream: pass2 now reads pass1's `Any`.
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p2, 0)),
        DataType::Any
    );

    // A scalar const carries its type, so the output resolves to it (and
    // propagates downstream) — a const isn't "no type".
    graph.set_input_binding(
        InputPort::new(p1, 0),
        Binding::Const(StaticValue::Bool(true)),
    );
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p1, 0)),
        DataType::Bool
    );
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p2, 0)),
        DataType::Bool,
        "the const's type propagates through the second passthrough too"
    );

    // A const whose type can't be reconstructed from the value alone — an
    // enum literal on a `Any` (wildcard) input — stays polymorphic rather
    // than panicking. (The passthrough's value input is `Any`-declared.)
    graph.set_input_binding(
        InputPort::new(p1, 0),
        Binding::Const(StaticValue::Enum("X".into())),
    );
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(p1, 0)),
        DataType::Any
    );
}

#[test]
fn resolve_output_type_uses_declared_type_for_typed_const_input() {
    use crate::library::Library;
    use crate::{DataType, FsPathConfig, FsPathMode, StaticValue, TypeId};
    use std::sync::Arc;

    // A reroute func with *typed* inputs, each mirrored by a wildcard output.
    let fs_ty = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)));
    let enum_ty = DataType::Enum(TypeId::from_u128(0x5e));
    let func = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "reroute")
            .input(FuncInput::required("path", fs_ty.clone()))
            .input(FuncInput::required("mode", enum_ty.clone()))
            .wildcard_output("path_out", 0)
            .wildcard_output("mode_out", 1),
    );
    let mut library = Library::default();
    library.add(func.clone());

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
    assert_eq!(output_type(&graph, &library, OutputPort::new(n, 0)), fs_ty);
    assert_eq!(
        output_type(&graph, &library, OutputPort::new(n, 1)),
        enum_ty
    );
}

#[test]
fn type_mismatched_wiring_flattens_as_unbound_through_wildcard_chains() {
    use crate::DataType;
    use crate::execution::compile::Compiler;
    use crate::execution::compiled::ExecutionBinding;
    use crate::execution::identity::ExecutionNodeId;
    use crate::library::Library;

    let float_src = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float)),
    );
    let str_src = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String)),
    );
    let float_sink = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "fsink")
            .input(FuncInput::required("x", DataType::Float))
            .output(FuncOutput::new("o", DataType::Float)),
    );
    let pass_func = passthrough_func();
    let mut library = Library::default();
    library.add(float_src.clone());
    library.add(str_src.clone());
    library.add(float_sink.clone());
    library.add(pass_func.clone());

    let add_pass = |g: &mut Graph| g.add_func_node(&pass_func);

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

    // The sink's flat input in the compiled program: the type gate rules on
    // the authored wire, never on the document (nothing is severed). A `Bind`
    // is mapped back to its producer's id so assertions stay id-based.
    #[derive(Debug, PartialEq)]
    enum FlatSink {
        Unbound,
        Const,
        Bound(ExecutionNodeId),
    }
    let sink_binding = |g: &Graph| {
        let mut compiler = Compiler::default();
        let compiled = compiler.compile(g, &library).expect("mismatches compile");
        let e_node = &compiled[compiled.e_node_index[&ExecutionNodeId::from_node(sink)]];
        match &compiled.inputs[e_node.inputs.nth(0)].binding {
            ExecutionBinding::Bind(addr) => FlatSink::Bound(compiled.e_node_ids[addr.node_idx]),
            ExecutionBinding::Const(_) => FlatSink::Const,
            ExecutionBinding::None => FlatSink::Unbound,
        }
    };

    // The valid Float chain binds the sink to its passthrough producer
    // (passthroughs are real func nodes — only boundaries short-circuit).
    assert_eq!(
        sink_binding(&g),
        FlatSink::Bound(ExecutionNodeId::from_node(p2)),
        "a well-typed chain flattens as bound"
    );

    // Rewire pass1's value input to the String producer: pass1.out and
    // pass2.out both retype to String, so the *two-hops-down* sink edge is
    // the one now incompatible — it flattens as unbound while the authored
    // wire survives in the document.
    g.set_input_binding(InputPort::new(p1, 0), Binding::bind(sp, 0));
    assert_eq!(sink_binding(&g), FlatSink::Unbound);
    assert_eq!(
        g.bindings.get(&InputPort::new(sink, 0)),
        Some(&Binding::bind(p2, 0)),
        "the mismatched wire stays authored"
    );

    // A const that doesn't satisfy its port degrades the same way.
    g.set_input_binding(
        InputPort::new(sink, 0),
        Binding::Const(StaticValue::String("nope".into())),
    );
    assert_eq!(sink_binding(&g), FlatSink::Unbound);
    g.set_input_binding(
        InputPort::new(sink, 0),
        Binding::Const(StaticValue::Float(1.0)),
    );
    assert_eq!(sink_binding(&g), FlatSink::Const);
}

#[test]
fn resolve_output_type_breaks_a_binding_cycle() {
    use crate::DataType;
    use crate::library::Library;
    // A passthrough whose value input binds to its own output — a cycle the
    // editor can momentarily hold. Resolution must terminate as `Any`.
    let pass_func = passthrough_func();
    let mut library = Library::default();
    library.add(pass_func.clone());
    let mut graph = Graph::default();
    let id = graph.add_func_node(&pass_func);
    graph.set_input_binding(InputPort::new(id, 0), Binding::bind(id, 0));

    assert_eq!(
        output_type(&graph, &library, OutputPort::new(id, 0)),
        DataType::Any
    );
}

#[test]
fn node_remove_test() -> TestResult {
    let mut graph = test_graph();

    let node_id = graph.find_by_name("sum").unwrap().id;
    graph.find_mut(node_id).unwrap().cache = CacheMode::Ram;
    assert_eq!(graph.find_by_name("sum").unwrap().cache, CacheMode::Ram);
    for node in graph.nodes.values_mut() {
        node.disabled = true;
    }
    assert!(graph.iter().all(|node| node.disabled));

    graph.detach_node(node_id);

    assert!(graph.find_by_name("sum").is_none());
    assert_eq!(graph.len(), 4);

    // No surviving edge references the removed node (as consumer or producer).
    for (dst, src) in graph.edges() {
        assert_ne!(dst.node_id, node_id);
        assert_ne!(src.node_id, node_id);
    }

    Ok(())
}

/// The rule the editor applies before it lets a wire land: a back-edge closes
/// a loop, a forward or sideways one does not, and a node wired to itself
/// always does. Direct *and* transitive, since the walk is what tells the two
/// apart.
#[test]
fn produces_cycle_detects_direct_and_transitive_loops() {
    // A passthrough is both consumer and producer, so it can chain:
    // a → b → c, with d left unconnected.
    let relay = passthrough_func();
    let mut graph = Graph::default();
    let a = graph.add_func_node(&relay);
    let b = graph.add_func_node(&relay);
    let c = graph.add_func_node(&relay);
    let d = graph.add_func_node(&relay);
    graph.set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    graph.set_input_binding(InputPort::new(c, 0), Binding::bind(b, 0));

    assert!(graph.produces_cycle(b, a), "b → a closes a → b");
    assert!(
        graph.produces_cycle(c, a),
        "c → a closes a → b → c transitively"
    );
    assert!(graph.produces_cycle(a, a), "a node wired to itself");

    // Forward and sideways edges are fine: a second a → c path is a DAG
    // diamond, and an unconnected node is reachable from nothing.
    assert!(
        !graph.produces_cycle(a, c),
        "a → c is a second forward path"
    );
    assert!(!graph.produces_cycle(c, d), "d reads from nothing");
    assert!(!graph.produces_cycle(a, d), "d reads from nothing");
}

#[test]
fn typed_id_from_str_preserves_uuid_error() {
    let input = "not-a-uuid";
    let error: uuid::Error = input.parse::<FuncId>().unwrap_err();
    assert_eq!(
        error.to_string(),
        uuid::Uuid::parse_str(input).unwrap_err().to_string()
    );
}

#[test]
fn binding_conversions() {
    let nid = NodeId::unique();
    let from_port: Binding = OutputPort::new(nid, 1).into();
    assert_eq!(from_port, Binding::bind(nid, 1));
    assert_eq!(from_port, Binding::Bind(OutputPort::new(nid, 1)));

    let from_value: Binding = StaticValue::Int(7).into();
    assert_eq!(from_value, Binding::Const(StaticValue::Int(7)));
}

#[test]
fn input_bindings_are_sparse_and_none_removes_an_entry() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum").unwrap().id;
    let get_a_id = graph.find_by_name("get_a").unwrap().id;
    let get_b_id = graph.find_by_name("get_b").unwrap().id;

    let first = InputPort::new(sum_id, 0);
    let second = InputPort::new(sum_id, 1);
    let absent = InputPort::new(sum_id, 2);
    assert_eq!(
        graph.bindings.get(&first),
        Some(&Binding::bind(get_a_id, 0))
    );
    assert_eq!(
        graph.bindings.get(&second),
        Some(&Binding::bind(get_b_id, 0))
    );
    assert!(!graph.bindings.contains_key(&absent));

    let binding_count = graph.bindings.len();
    graph.set_input_binding(first, None);
    assert!(!graph.bindings.contains_key(&first));
    assert_eq!(graph.bindings.len(), binding_count - 1);
}

#[test]
fn subscribe_unsubscribe_is_subscribed() {
    let graph = test_graph();
    let emitter = graph.find_by_name("get_a").unwrap().id;
    let sub = graph.find_by_name("sum").unwrap().id;
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
    let emitter = graph.find_by_name("get_a").unwrap().id;
    let s1 = graph.find_by_name("sum").unwrap().id;
    let s2 = graph.find_by_name("mult").unwrap().id;
    let other = graph.find_by_name("Print").unwrap().id;

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

#[test]
fn wiring_snapshot_round_trips_through_serde_and_restore() -> TestResult {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum").unwrap().id;
    let get_a_id = graph.find_by_name("get_a").unwrap().id;
    // Add a subscription that touches `sum` so both arms are exercised.
    graph.subscribe(get_a_id, 0, sum_id);

    let bindings = graph.bindings_touching(sum_id);

    assert_eq!(bindings.len(), 3);

    let before = graph.clone_verbatim();
    let edges_before = graph.edges().count();
    let detached = graph.detach_node(sum_id);
    assert_eq!(graph.edges().count(), edges_before - 3);
    assert!(!graph.is_subscribed(get_a_id, 0, sum_id));

    let serialized = serialize(&detached, SerdeFormat::Bitcode)?;
    let decoded: DetachedNode = deserialize(&serialized, SerdeFormat::Bitcode)?;
    assert_eq!(decoded, detached);

    let mut nil_id = detached.clone();
    nil_id.node_id = NodeId::nil();
    let mut mismatched = detached.clone();
    mismatched.node_id = NodeId::unique();
    for invalid in [nil_id, mismatched] {
        let serialized = serialize(&invalid, SerdeFormat::Json)?;
        let decoded_invalid: DetachedNode = deserialize(&serialized, SerdeFormat::Json)?;
        let detached_graph = graph.clone_verbatim();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph.attach_node(decoded_invalid);
        }));
        assert!(result.is_err());
        assert_eq!(graph, detached_graph, "failed attachment mutated the graph");
    }

    graph.attach_node(decoded);

    assert_eq!(graph, before);

    Ok(())
}

fn func_with_default(default: i64) -> Func {
    Func::new(FuncId::unique(), "withdefault")
        .input(FuncInput::optional("x", DataType::Int).default(default))
}

#[test]
fn add_func_node_seeds_default_const_binding() {
    let func = func_with_default(7);
    let mut graph = Graph::default();
    let id = graph.add_func_node(&func);

    assert_eq!(graph.find(id).unwrap().kind, NodeKind::Func(func.id));
    assert_eq!(
        graph.bindings.get(&InputPort::new(id, 0)),
        Some(&Binding::Const(7i64.into()))
    );
}

#[test]
fn add_func_node_leaves_defaultless_inputs_unbound() {
    let library = test_func_lib(TestFuncHooks::default());
    let sum = library.by_name("sum").unwrap(); // inputs have no defaults
    let mut graph = Graph::default();
    let id = graph.add_func_node(sum);

    assert!(!graph.bindings.contains_key(&InputPort::new(id, 0)));
    assert!(!graph.bindings.contains_key(&InputPort::new(id, 1)));
}

#[test]
fn serialization_round_trips_a_graph_through_every_format() -> TestResult {
    let graph = test_graph();
    for format in SerdeFormat::all_formats_for_testing() {
        let serialized = graph.serialize(format)?;
        let deserialized = Graph::deserialize(&serialized, format)?;
        assert_eq!(graph, deserialized, "{format:?} round-trips a graph whole");
    }
    Ok(())
}

/// Deserialization is the release-path structural guard on an untrusted
/// document: `serialize` doesn't validate, so every structural invariant the
/// graph's own mutations assert on has to be *checked* on the way back in.
#[test]
fn deserialize_rejects_corrupt_graph() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum").unwrap().id;
    graph.set_input_binding(
        InputPort::new(sum_id, 0),
        Binding::bind(NodeId::unique(), 0),
    );
    let bytes = graph.serialize(SerdeFormat::Bitcode).unwrap();
    assert!(
        matches!(
            Graph::deserialize(&bytes, SerdeFormat::Bitcode),
            Err(GraphDeserializeError::InvalidGraph(
                GraphValidationError::BindingMissingProducer { .. }
            ))
        ),
        "a binding naming a node the document doesn't hold is rejected"
    );

    let mut nil_key = Graph::default();
    nil_key
        .nodes
        .insert(NodeId::nil(), Node::new(NodeKind::Func(FuncId::unique())));
    let bytes = nil_key.serialize(SerdeFormat::Bitcode).unwrap();
    assert!(matches!(
        Graph::deserialize(&bytes, SerdeFormat::Bitcode),
        Err(GraphDeserializeError::InvalidGraph(
            GraphValidationError::NilNodeId
        ))
    ));

    // Bindings decode from a sequence into a map, so a repeated input port is
    // caught during decode rather than by validation after it.
    let mut duplicate_bindings = serde_json::to_value(test_graph()).unwrap();
    let bindings = duplicate_bindings["bindings"].as_array_mut().unwrap();
    bindings.push(bindings[0].clone());
    let bytes = serde_json::to_vec(&duplicate_bindings).unwrap();
    let error = Graph::deserialize(&bytes, SerdeFormat::Json).unwrap_err();
    assert!(matches!(&error, GraphDeserializeError::Deserialize(_)));
    assert!(
        error
            .to_string()
            .contains("duplicate binding for input port"),
        "{error}"
    );
}

/// Wiring the current library can't resolve is tolerated, never a validation
/// error: it degrades to unbound at flatten time and revives if the library
/// grows the ports back. The counterpart to
/// [`type_mismatches_degrade_at_flatten_not_at_validation`].
#[test]
fn validate_tolerates_library_range_drift() {
    let func = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "one_out").output(FuncOutput::new("o", DataType::Int)),
    );
    let mut library = Library::default();
    library.add(func.clone());

    let mut graph = Graph::default();
    let id = graph.add_func_node(&func);
    assert!(graph.validate_with(&library).is_ok());

    // Input 5, output 7, and event 3 are all past what `one_out` declares.
    graph.set_input_binding(InputPort::new(id, 5), Binding::bind(id, 7));
    graph.subscribe(id, 3, id);
    assert!(graph.validate_with(&library).is_ok());

    // `Null` consts ("explicitly unset") are tolerated on both sides:
    // meaningful on an optional input, degrading to a missing input on a
    // required one at flatten (see `const_satisfies`).
    let nullable = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "nullable")
            .input(FuncInput::optional("opt", DataType::Int))
            .input(FuncInput::required("req", DataType::Int))
            .output(FuncOutput::new("o", DataType::Int)),
    );
    library.add(nullable.clone());
    let node = graph.add_func_node(&nullable);
    graph.set_input_binding(InputPort::new(node, 0), Binding::Const(StaticValue::Null));
    graph.set_input_binding(InputPort::new(node, 1), Binding::Const(StaticValue::Null));
    assert!(graph.validate_with(&library).is_ok());
}

/// `Some(ports)` is an authoritative arity a caller may range-check against;
/// `None` means "unknowable here" and must *not* read as an empty port list —
/// the drift guards do `is_some_and(|p| idx >= p.len())`, so the two decide
/// opposite ways.
#[test]
fn node_ports_resolve_to_a_declaration_or_to_unknown() {
    let library = test_func_lib(TestFuncHooks::default());
    let mut graph = Graph::default();

    let sum = library.by_name("sum").unwrap();
    let sum_id = graph.add_func_node(sum);
    let sum_node = graph.find(sum_id).unwrap();
    let ports = graph.node_ports(sum_node, &library).unwrap();
    assert_eq!(ports.name, "sum");
    assert_eq!(ports.inputs.len(), sum.inputs.len());
    assert_eq!(ports.func.id, sum.id);

    // Library drift is unknown, not empty — otherwise every port on a node
    // whose func went missing would read as out of range.
    let missing_func = Node::new(NodeKind::Func(FuncId::unique()));
    assert!(graph.node_ports(&missing_func, &library).is_none());

    // The three policy flags come off the declaration verbatim. Each is set
    // on one of the two funcs and clear on the other, so a flag wired to the
    // wrong field can't pass.
    let plain = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "plain")
            .pure()
            .output(FuncOutput::new("o", DataType::Int)),
    );
    let plain = plain.ports();
    assert!(!plain.sink());
    assert!(!plain.uncacheable());
    assert!(!plain.impure(), "declared `pure` is not impure");

    let flagged = Func::new(FuncId::unique(), "flagged")
        .sink()
        .uncacheable()
        .input(FuncInput::optional("v", DataType::Int));
    let flagged = flagged.ports();
    assert!(flagged.sink());
    assert!(flagged.uncacheable());
    assert!(
        flagged.impure(),
        "`Func::new` leaves a func Impure until `pure()` says otherwise"
    );
}

#[test]
fn input_type_resolves_declared_types_and_rejects_out_of_range() {
    let consumer = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "dst")
            .input(FuncInput::required("x", DataType::Float))
            .output(FuncOutput::new("out", DataType::Float)),
    );
    let mut library = Library::default();
    library.add(consumer.clone());

    let mut graph = Graph::default();
    let dst = graph.add_func_node(&consumer);
    assert_eq!(
        graph.input_type(&library, InputPort::new(dst, 0)),
        Some(DataType::Float)
    );
    assert_eq!(graph.input_type(&library, InputPort::new(dst, 9)), None);
}

#[test]
fn node_events_expose_names_and_arity() {
    let emitter = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "ticker")
            .event("tick", EventLambda::default())
            .event("tock", EventLambda::default()),
    );
    let ports = emitter.ports();
    assert_eq!(ports.events.len(), 2);
    let names: Vec<&str> = ports.events.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["tick", "tock"]);

    let silent = testing::with_stub_lambda(Func::new(FuncId::unique(), "silent"));
    assert!(silent.ports().events.is_empty());
}
