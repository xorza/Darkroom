use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{
    Binding, CacheMode, Graph, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort,
};
use crate::library::Library;
use crate::node::definition::{Func, FuncId, FuncInput, FuncOutput};
use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, DetachedNode, closes_data_cycle};
use common::{SerdeFormat, deserialize, serialize};

/// A passthrough func — one `Any` input, one wildcard output mirroring it. The
/// generic hop for testing wildcard type resolution through a node.
fn passthrough_func() -> Func {
    Func::new(FuncId::unique(), "pass")
        .input(FuncInput::required("x", DataType::Any))
        .wildcard_output("o", 0)
}

#[test]
fn roundtrip_serialization() -> anyhow::Result<()> {
    let graph = test_graph();

    for format in SerdeFormat::all_formats_for_testing() {
        let serialized = graph.serialize(format)?;
        let deserialized = Graph::deserialize(&serialized, format)?;
        assert_eq!(graph, deserialized);
    }

    Ok(())
}

#[test]
fn check_rejects_node_ids_reused_across_graph_levels() {
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let node_id = NodeId::unique();
    let mut interior = Graph::default();
    interior.insert(node_id, node.clone());
    let graph_id = GraphId::unique();
    interior.name = "duplicate id".into();

    let mut graph = Graph::default();
    graph.insert(node_id, node);
    graph.insert_graph(graph_id, interior);

    let error = graph.check().unwrap_err().to_string();
    assert!(error.contains("occurs in more than one authoring graph"));
}

#[test]
fn insert_graph_replaces_existing_graph() {
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, Graph::new("original"));
    graph.insert_graph(graph_id, Graph::new("replacement"));
    assert_eq!(graph.graphs[&graph_id].name, "replacement");
}

#[test]
fn pinned_outputs_roundtrip_serialization() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    graph.set_output_pinned(OutputPort::new(sum_id, 0), true);

    for format in SerdeFormat::all_formats_for_testing() {
        let serialized = graph.serialize(format)?;
        let deserialized = Graph::deserialize(&serialized, format)?;
        assert!(deserialized.is_output_pinned(OutputPort::new(sum_id, 0)));
        assert_eq!(graph, deserialized);
    }

    Ok(())
}

#[test]
fn check_passes_for_valid_graph() {
    assert!(test_graph().check().is_ok());
}

#[test]
fn check_accepts_reusable_graph_while_compile_check_rejects_it_as_entry() {
    let mut graph = Graph::new("reusable")
        .input(FuncInput::optional("value", DataType::Int))
        .output(FuncOutput::new("result", DataType::Int));
    graph.add(Node::new(NodeKind::GraphInput));
    graph.add(Node::new(NodeKind::GraphOutput));

    assert!(graph.check().is_ok());
    let error = graph.check_for_execution(&Default::default()).unwrap_err();
    assert!(error.to_string().contains("entry graph"));
}

#[test]
fn check_for_execution_validates_shared_graph_structure_and_recursion() {
    let graph_id = GraphId::unique();
    let mut shared = Graph::new("recursive");
    shared.add(Node::new(NodeKind::Graph(GraphLink::Shared(graph_id))));

    let mut library = Library::default();
    library.insert_graph(graph_id, shared);

    let mut graph = Graph::default();
    graph.add(Node::new(NodeKind::Graph(GraphLink::Shared(graph_id))));

    let error = graph.check_for_execution(&library).unwrap_err().to_string();
    assert!(error.contains("recursive"));

    let graph_id = GraphId::unique();
    let mut shared = Graph::new("structurally invalid");
    shared.add(Node::new(NodeKind::GraphInput));
    shared.add(Node::new(NodeKind::GraphInput));

    let mut library = Library::default();
    library.insert_graph(graph_id, shared);

    let mut graph = Graph::default();
    graph.add(Node::new(NodeKind::Graph(GraphLink::Shared(graph_id))));

    let error = graph.check_for_execution(&library).unwrap_err().to_string();
    assert!(error.contains("at most one GraphInput"));
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
                back.find_by_name("get_a", NodeSearch::TopLevel)
                    .unwrap()
                    .cache,
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
    assert_eq!(
        graph.find(&id, NodeSearch::TopLevel).unwrap().cache,
        CacheMode::Both
    );

    // The func-less constructors have no func to copy from and seed `None`.
    assert_eq!(
        Node::new(NodeKind::Func(FuncId::unique())).cache,
        CacheMode::None
    );
}

#[test]
fn check_rejects_dangling_binding() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    // Repoint sum's input at a node that doesn't exist.
    graph.set_input_binding(
        InputPort::new(sum_id, 0),
        Binding::bind(NodeId::unique(), 0),
    );

    let err = graph.check().expect_err("dangling binding must fail check");
    assert!(err.to_string().contains("binds to missing node"));
}

#[test]
fn const_only_input_rejects_bind_but_a_normal_input_accepts_it() {
    use crate::library::Library;
    use crate::node::definition::FuncId;

    // One Int-in / Int-out func, so a wire between two instances is otherwise
    // valid — only the `const_only` flag decides whether check accepts it.
    let check = |const_only: bool| -> anyhow::Result<()> {
        let port = FuncInput::required("locked", DataType::Int);
        let port = if const_only { port.const_only() } else { port };
        let func = Func::new(FuncId::unique(), "f")
            .input(port)
            .output(FuncOutput::new("out", DataType::Int));
        let mut library = Library::default();
        library.add(func.clone());

        let mut graph = Graph::default();
        let producer = graph.add_func_node(&func);
        let consumer = graph.add_func_node(&func);
        graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
        graph.check_for_execution(&library)
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
fn check_for_execution_rejects_type_mismatched_bindings_through_passthroughs() {
    use crate::library::Library;
    use crate::{DataType, StaticValue};

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
    let pass_func = passthrough_func();
    let mut library = Library::default();
    library.add(int_src.clone());
    library.add(str_sink.clone());
    library.add(int_sink.clone());
    library.add(pass_func.clone());

    // Direct Int → String is rejected at the compile boundary.
    let mut g = Graph::default();
    let s = g.add_func_node(&int_src);
    let f = g.add_func_node(&str_sink);
    g.set_input_binding(InputPort::new(f, 0), Binding::bind(s, 0));
    let err = g
        .check_for_execution(&library)
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
    assert!(g.check_for_execution(&library).is_ok());

    // The check resolves *through* a passthrough: Int → pass → Int is fine,
    // Int → pass → String is rejected (the wildcard carries the real type).
    let mut g = Graph::default();
    let s = g.add_func_node(&int_src);
    let pid = g.add_func_node(&pass_func);
    g.set_input_binding(InputPort::new(pid, 0), Binding::bind(s, 0));
    let i = g.add_func_node(&int_sink);
    g.set_input_binding(InputPort::new(i, 0), Binding::bind(pid, 0));
    assert!(
        g.check_for_execution(&library).is_ok(),
        "Int through a passthrough into Int is compatible"
    );

    g.set_input_binding(InputPort::new(i, 0), None);
    let f = g.add_func_node(&str_sink);
    g.set_input_binding(InputPort::new(f, 0), Binding::bind(pid, 0));
    assert!(
        g.check_for_execution(&library)
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
        g.check_for_execution(&library)
            .is_err_and(|e| e.to_string().contains("incompatible")),
        "a String constant on an Int input must be rejected"
    );
    g.set_input_binding(
        InputPort::new(i, 0),
        Binding::Const(StaticValue::Float(2.5)),
    );
    assert!(
        g.check_for_execution(&library).is_ok(),
        "a numeric constant satisfies a numeric input"
    );
}

#[test]
fn check_for_execution_rejects_out_of_range_pinned_output() {
    use crate::library::Library;

    let func = Func::new(FuncId::unique(), "one_out").output(FuncOutput::new("o", DataType::Int));
    let mut library = Library::default();
    library.add(func.clone());

    let mut graph = Graph::default();
    let id = graph.add_func_node(&func);

    graph.set_output_pinned(OutputPort::new(id, 0), true);
    assert!(graph.check_for_execution(&library).is_ok());

    graph.set_output_pinned(OutputPort::new(id, 1), true);
    let err = graph
        .check_for_execution(&library)
        .expect_err("output 1 doesn't exist on a one-output func");
    assert!(err.to_string().contains("out of range"), "{err}");
}

#[test]
fn resolve_output_type_follows_passthrough_chain() {
    use crate::library::Library;
    use crate::{DataType, StaticValue};

    // Int-out producer → pass1 → pass2. Both passthroughs declare a `Any`
    // (wildcard) output, but the resolved type must be the producer's `Int`.
    let producer = Func::new(FuncId::unique(), "src").output(FuncOutput::new("out", DataType::Int));
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

    // An unbound value input leaves the passthrough polymorphic (`Any`),
    // so its output accepts any consumer again.
    graph.set_input_binding(InputPort::new(p1, 0), None);
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p1, 0)),
        DataType::Any
    );
    // The taint flows downstream: pass2 now reads pass1's `Any`.
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p2, 0)),
        DataType::Any
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
    // enum literal on a `Any` (wildcard) input — stays polymorphic rather
    // than panicking. (The passthrough's value input is `Any`-declared.)
    graph.set_input_binding(
        InputPort::new(p1, 0),
        Binding::Const(StaticValue::Enum("X".into())),
    );
    assert_eq!(
        graph.resolve_output_type(&library, OutputPort::new(p1, 0)),
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
    let func = Func::new(FuncId::unique(), "reroute")
        .input(FuncInput::required("path", fs_ty.clone()))
        .input(FuncInput::required("mode", enum_ty.clone()))
        .wildcard_output("path_out", 0)
        .wildcard_output("mode_out", 1);
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
    use crate::DataType;
    use crate::library::Library;

    let float_src =
        Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float));
    let str_src =
        Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String));
    let float_sink = Func::new(FuncId::unique(), "fsink")
        .input(FuncInput::required("x", DataType::Float))
        .output(FuncOutput::new("o", DataType::Float));
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

    // Rewire pass1's value input to the String producer: pass1.out and
    // pass2.out both retype to String, so the *two-hops-down* sink edge is
    // the one now incompatible — the chain must be followed to find it.
    g.set_input_binding(InputPort::new(p1, 0), Binding::bind(sp, 0));
    assert_eq!(
        g.edges_invalidated_by(&library, InputPort::new(p1, 0)),
        vec![InputPort::new(sink, 0)],
        "the edge two passthroughs downstream is flagged"
    );

    // Changing an ordinary node's input retypes nothing → no invalidations.
    assert!(
        g.edges_invalidated_by(&library, InputPort::new(sink, 0))
            .is_empty()
    );
    // Changing the passthrough's *path* input (no output mirrors it) → none.
    assert!(
        g.edges_invalidated_by(&library, InputPort::new(p1, 1))
            .is_empty()
    );
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
        graph.resolve_output_type(&library, OutputPort::new(id, 0)),
        DataType::Any
    );
}

#[test]
fn input_type_resolves_declared_types_and_skips_boundaries() {
    use crate::DataType;
    use crate::library::Library;

    let consumer = Func::new(FuncId::unique(), "dst")
        .input(FuncInput::required("x", DataType::Float))
        .output(FuncOutput::new("out", DataType::Float));
    let mut library = Library::default();
    library.add(consumer.clone());

    let mut graph = Graph::default();
    let dst = graph.add_func_node(&consumer);
    assert_eq!(
        graph.input_type(&library, InputPort::new(dst, 0)),
        Some(DataType::Float)
    );
    // Out-of-range port → None.
    assert_eq!(graph.input_type(&library, InputPort::new(dst, 9)), None);

    // A boundary node carries no per-port type here → None (caller's Null).
    let boundary = Node::new(NodeKind::GraphInput);
    let b = graph.add(boundary);
    assert_eq!(graph.input_type(&library, InputPort::new(b, 0)), None);
}

#[test]
fn deserialize_rejects_corrupt_graph() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    graph.set_input_binding(
        InputPort::new(sum_id, 0),
        Binding::bind(NodeId::unique(), 0),
    );

    // serialize doesn't validate; deserialize must reject the dangling bind
    // (the release-path structural guard, not a debug-only assert).
    let bytes = graph.serialize(SerdeFormat::Bitcode).unwrap();
    assert!(Graph::deserialize(&bytes, SerdeFormat::Bitcode).is_err());

    let mut nil_key = Graph::default();
    nil_key
        .nodes
        .insert(NodeId::nil(), Node::new(NodeKind::Func(FuncId::unique())));
    let bytes = nil_key.serialize(SerdeFormat::Bitcode).unwrap();
    assert!(Graph::deserialize(&bytes, SerdeFormat::Bitcode).is_err());

    let nil_origin = Graph {
        origin: Some(GraphId::nil()),
        ..Graph::default()
    };
    let bytes = nil_origin.serialize(SerdeFormat::Bitcode).unwrap();
    let error = Graph::deserialize(&bytes, SerdeFormat::Bitcode)
        .unwrap_err()
        .to_string();
    assert!(error.contains("graph has a nil origin"), "{error}");

    let mut duplicate_bindings = serde_json::to_value(test_graph()).unwrap();
    let bindings = duplicate_bindings["bindings"].as_array_mut().unwrap();
    bindings.push(bindings[0].clone());
    let bytes = serde_json::to_vec(&duplicate_bindings).unwrap();
    let error = Graph::deserialize(&bytes, SerdeFormat::Json)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("duplicate binding for input port"),
        "{error}"
    );
}

#[test]
fn node_remove_test() -> anyhow::Result<()> {
    let mut graph = test_graph();

    let node_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    graph
        .find_mut(&node_id, NodeSearch::TopLevel)
        .unwrap()
        .cache = CacheMode::Ram;
    assert_eq!(
        graph
            .find_by_name("sum", NodeSearch::TopLevel)
            .unwrap()
            .cache,
        CacheMode::Ram
    );
    for node in graph.nodes.values_mut() {
        node.disabled = true;
    }
    assert!(graph.iter().all(|node| node.disabled));

    graph.set_output_pinned(OutputPort::new(node_id, 0), true);
    graph.detach_node(node_id);

    assert!(graph.find_by_name("sum", NodeSearch::TopLevel).is_none());
    assert_eq!(graph.len(), 4);

    // No surviving edge references the removed node (as consumer or producer).
    for (dst, src) in graph.edges() {
        assert_ne!(dst.node_id, node_id);
        assert_ne!(src.node_id, node_id);
    }

    // Nor does a pin on one of its own output ports.
    assert!(!graph.is_output_pinned(OutputPort::new(node_id, 0)));

    Ok(())
}

#[test]
fn node_kind_accessors() {
    let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
    let func = NodeKind::Func(func_id);
    assert_eq!(func.as_func(), Some(func_id));
    assert_eq!(func.as_graph(), None);
    assert!(!func.is_boundary());

    let graph_id = GraphId::unique();
    let sub = NodeKind::Graph(GraphLink::Local(graph_id));
    assert_eq!(sub.as_func(), None);
    assert_eq!(sub.as_graph().map(|r| r.id()), Some(graph_id));
    assert!(!sub.is_boundary());

    assert!(NodeKind::GraphInput.is_boundary());
    assert!(NodeKind::GraphOutput.is_boundary());
    assert_eq!(NodeKind::GraphInput.as_func(), None);
    assert_eq!(NodeKind::GraphOutput.as_graph(), None);
}

#[test]
fn node_func_id_shims_kind() {
    let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
    assert_eq!(Node::new(NodeKind::Func(func_id)).func_id(), Some(func_id));
    assert_eq!(Node::new(NodeKind::GraphInput).func_id(), None);
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
fn binding_accessors() {
    let out = OutputPort::new(NodeId::unique(), 2);
    let bind = Binding::Bind(out);
    assert_eq!(bind.as_output_binding(), Some(&out));

    let konst = Binding::from(5i64);
    assert_eq!(konst.as_output_binding(), None);
}

#[test]
fn binding_conversions() {
    let nid = NodeId::unique();
    let from_port: Binding = OutputPort::new(nid, 1).into();
    assert_eq!(from_port, Binding::bind(nid, 1));
    assert_eq!(from_port.as_output_binding().unwrap().port_idx, 1);

    assert_eq!(Binding::from(7i64), Binding::Const(7i64.into()));
}

#[test]
fn input_bindings_are_sparse_and_none_removes_an_entry() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    let get_a_id = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let get_b_id = graph
        .find_by_name("get_b", NodeSearch::TopLevel)
        .unwrap()
        .id;

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
    let emitter = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let sub = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
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
    let emitter = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let s1 = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    let s2 = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
    let other = graph
        .find_by_name("Print", NodeSearch::TopLevel)
        .unwrap()
        .id;

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
fn set_output_pinned_and_is_output_pinned() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    let port = OutputPort::new(sum_id, 0);

    assert!(!graph.is_output_pinned(port));
    graph.set_output_pinned(port, true);
    assert!(graph.is_output_pinned(port));

    // A distinct port on the same node is a distinct flag.
    assert!(!graph.is_output_pinned(OutputPort::new(sum_id, 1)));

    // Re-marking is idempotent (BTreeSet dedups).
    graph.set_output_pinned(port, true);

    graph.set_output_pinned(port, false);
    assert!(!graph.is_output_pinned(port));
}

#[test]
fn fresh_copy_remaps_pinned_outputs() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    graph.set_output_pinned(OutputPort::new(sum_id, 0), true);

    let fresh = graph.fresh_copy();
    let new_sum_id = fresh.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;

    assert!(!fresh.is_output_pinned(OutputPort::new(sum_id, 0)));
    assert!(fresh.is_output_pinned(OutputPort::new(new_sum_id, 0)));
}

#[test]
fn wiring_snapshot_round_trips_through_serde_and_restore() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    let get_a_id = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
    let pinned = OutputPort::new(sum_id, 0);

    // Add a subscription that touches `sum` so both arms are exercised.
    graph.subscribe(get_a_id, 0, sum_id);
    graph.set_output_pinned(pinned, true);

    let bindings = graph.bindings_touching(sum_id);

    assert_eq!(bindings.len(), 3);

    let before = graph.clone();
    let edges_before = graph.edges().count();
    let detached = graph.detach_node(sum_id);
    assert_eq!(graph.edges().count(), edges_before - 3);
    assert!(!graph.is_subscribed(get_a_id, 0, sum_id));
    assert!(!graph.is_output_pinned(pinned));

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
        let detached_graph = graph.clone();
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

    assert_eq!(
        graph.find(&id, NodeSearch::TopLevel).unwrap().func_id(),
        Some(func.id)
    );
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
fn add_graph_node_seeds_default_const_binding() {
    let mut input = FuncInput::optional("A", DataType::Int).default(3i64);
    let graph_id = GraphId::unique();
    let def = Graph::new("Def").category("Test").inputs([input.clone(), {
        input.default_value = None;
        input
    }]);

    let mut graph = Graph::default();
    let id = graph.add_graph_node(&def, GraphLink::Local(graph_id));

    // Port 0 had a default; port 1 did not.
    assert_eq!(
        graph.bindings.get(&InputPort::new(id, 0)),
        Some(&Binding::Const(3i64.into()))
    );
    assert!(!graph.bindings.contains_key(&InputPort::new(id, 1)));
}

#[test]
fn node_search_scope_gates_graph_interiors() {
    // A top-level node plus one two-levels-deep: a local graph whose
    // interior holds another local graph with the target node inside.
    let mut inner_graph = Graph::default();
    let mut deep = Node::new(NodeKind::Func(FuncId::unique()));
    deep.name = "deep".to_owned();
    let deep_id = inner_graph.add(deep);
    let inner_id = GraphId::unique();
    inner_graph.name = "Inner".into();

    let mut outer_graph = Graph::default();
    outer_graph.insert_graph(inner_id, inner_graph);
    let outer_id = GraphId::unique();
    outer_graph.name = "Outer".into();

    let mut graph = Graph::default();
    let mut top = Node::new(NodeKind::Func(FuncId::unique()));
    top.name = "top".to_owned();
    let top_id = graph.add(top);
    graph.insert_graph(outer_id, outer_graph);

    // Top-level node: found either way.
    assert!(graph.find(&top_id, NodeSearch::TopLevel).is_some());
    assert!(graph.find(&top_id, NodeSearch::Recursive).is_some());
    assert_eq!(
        graph.find_by_name("top", NodeSearch::TopLevel).unwrap().id,
        top_id
    );
    assert_eq!(
        graph.find_by_name("top", NodeSearch::Recursive).unwrap().id,
        top_id
    );
    // Interior node: invisible to TopLevel, found two levels down by
    // Recursive; an unknown id misses both ways.
    assert!(graph.find(&deep_id, NodeSearch::TopLevel).is_none());
    assert!(graph.find(&deep_id, NodeSearch::Recursive).is_some());
    assert!(graph.find_by_name("deep", NodeSearch::TopLevel).is_none());
    assert_eq!(
        graph
            .find_by_name("deep", NodeSearch::Recursive)
            .unwrap()
            .id,
        deep_id
    );
    assert!(
        graph
            .find(&NodeId::unique(), NodeSearch::Recursive)
            .is_none()
    );
    assert!(
        graph
            .find_by_name("missing", NodeSearch::Recursive)
            .is_none()
    );

    graph
        .find_mut(&deep_id, NodeSearch::Recursive)
        .unwrap()
        .name = "top".to_owned();
    assert_eq!(
        graph.find_by_name("top", NodeSearch::Recursive).unwrap().id,
        top_id
    );

    // The mutable lookup resolves identically and its edit lands on the
    // nested node.
    graph
        .find_mut(&deep_id, NodeSearch::Recursive)
        .unwrap()
        .name = "renamed".to_owned();
    assert_eq!(
        graph.find(&deep_id, NodeSearch::Recursive).unwrap().name,
        "renamed"
    );
    assert!(graph.find_by_name("deep", NodeSearch::Recursive).is_none());
    assert_eq!(
        graph
            .find_by_name("renamed", NodeSearch::Recursive)
            .unwrap()
            .id,
        deep_id
    );
    assert!(graph.find_mut(&deep_id, NodeSearch::TopLevel).is_none());
}

#[test]
fn resolve_graph_picks_local_or_linked_source() {
    let mut library = test_func_lib(TestFuncHooks::default());

    let linked_id = GraphId::unique();
    library.insert_graph(linked_id, Graph::new("Linked").category("Test"));

    let mut graph = Graph::default();
    let local_id = GraphId::unique();
    graph.insert_graph(local_id, Graph::new("Local").category("Test"));

    assert_eq!(
        graph
            .resolve_graph(GraphLink::Local(local_id), &library)
            .unwrap()
            .name,
        "Local"
    );
    assert_eq!(
        graph
            .resolve_graph(GraphLink::Shared(linked_id), &library)
            .unwrap()
            .name,
        "Linked"
    );
    // A local ref whose id only exists in the library does not resolve.
    assert!(
        graph
            .resolve_graph(GraphLink::Local(linked_id), &library)
            .is_none()
    );
}

#[test]
fn prune_subscriptions_drops_out_of_range_and_missing_emitter() {
    use crate::library::Library;
    use crate::node::event::EventLambda;

    // A func exposing exactly one event: idx 0 is valid, idx 1+ are not.
    let func_id = FuncId::from_u128(0xe0e0);
    let mut library = Library::default();
    library.add(Func::new(func_id, "emitter").event("tick", EventLambda::default()));

    let mut graph = Graph::default();
    let emitter = Node::new(NodeKind::Func(func_id));
    let subscriber = Node::new(NodeKind::Func(func_id));
    let emitter_id = graph.add(emitter);
    let subscriber_id = graph.add(subscriber);

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
    let ghost_id = graph.add(ghost);
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
            graph.add(n)
        })
        .collect();
    let (a, b, c, d, e) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let ghost = NodeId::unique();
    graph.inputs.push(FuncInput::optional("in", DataType::Int));
    graph.outputs.push(FuncOutput::new("out", DataType::Int));
    let graph_input = graph.add(Node::new(NodeKind::GraphInput));
    let graph_output = graph.add(Node::new(NodeKind::GraphOutput));

    graph.set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0)); // fully valid
    graph.set_input_binding(InputPort::new(c, 5), Binding::bind(a, 0)); // consumer input out of range
    graph.set_input_binding(InputPort::new(d, 0), Binding::bind(a, 9)); // producer output out of range
    graph.set_input_binding(InputPort::new(e, 0), Binding::bind(ghost, 0)); // producer node gone
    graph.set_input_binding(InputPort::new(ghost, 0), Binding::bind(a, 0)); // consumer node gone
    graph.set_input_binding(
        InputPort::new(graph_output, 0),
        Binding::bind(graph_input, 0),
    );
    graph.set_input_binding(
        InputPort::new(graph_output, 1),
        Binding::bind(graph_input, 0),
    );

    let removed = graph.prune_dangling_wiring(&library);
    assert_eq!(
        removed, 5,
        "every dangling binding drops, the valid one stays"
    );
    assert!(matches!(
        graph.bindings.get(&InputPort::new(b, 0)),
        Some(Binding::Bind(_))
    ));
    for dead in [
        InputPort::new(c, 5),
        InputPort::new(d, 0),
        InputPort::new(e, 0),
        InputPort::new(ghost, 0),
    ] {
        assert!(!graph.bindings.contains_key(&dead));
    }
    assert!(matches!(
        graph.bindings.get(&InputPort::new(graph_output, 0)),
        Some(Binding::Bind(_))
    ));
    assert!(
        !graph
            .bindings
            .contains_key(&InputPort::new(graph_output, 1))
    );

    // Const bindings are never structurally dangling — kept regardless.
    graph.set_input_binding(
        InputPort::new(b, 0),
        Binding::Const(crate::StaticValue::from(1i64)),
    );
    assert_eq!(graph.prune_dangling_wiring(&library), 0);

    // A node whose func is absent from the library (a stub for a doc saved
    // against a richer library) has unknowable arity, so its wiring is
    // kept — never panicked on, never dropped.
    let ghost_func = Node::new(NodeKind::Func(FuncId::from_u128(0xdead)));
    let ghost_id = graph.add(ghost_func);
    graph.set_input_binding(InputPort::new(ghost_id, 3), Binding::bind(a, 0));
    graph.set_input_binding(InputPort::new(b, 0), Binding::bind(ghost_id, 7));
    assert_eq!(
        graph.prune_dangling_wiring(&library),
        0,
        "unresolvable-node wiring is preserved"
    );
    assert!(matches!(
        graph.bindings.get(&InputPort::new(ghost_id, 3)),
        Some(Binding::Bind(_))
    ));
}
