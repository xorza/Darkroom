use crate::graph::Graph;
use crate::graph::definition::{GraphDef, GraphEvent, GraphId, GraphLink};
use crate::graph::error::{GraphDeserializeError, GraphValidationError};
use crate::graph::node::definition::{Func, FuncId, FuncInput, FuncOutput};
use crate::graph::node::event::EventLambda;
use crate::graph::node::{CacheMode, Node, NodeKind, NodeSearch};
use crate::graph::{Binding, BindingEntry, InputPort, NodeId, OutputPort};
use crate::library::Library;
use crate::testing::{self, TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, DetachedNode, StaticValue};
use common::{SerdeFormat, deserialize, serialize};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

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
fn roundtrip_serialization() -> TestResult {
    let graph = test_graph();

    for format in SerdeFormat::all_formats_for_testing() {
        let serialized = graph.serialize(format)?;
        let deserialized = Graph::deserialize(&serialized, format)?;
        assert_eq!(graph, deserialized);
    }

    let entry_json = serde_json::to_value(&graph)?;
    assert!(
        entry_json.get("name").is_none(),
        "an entry graph exposes nothing, so it serializes none of it"
    );

    // A definition serializes what it exposes flat, beside its body — the
    // two halves of one value, not a nested one.
    let subgraph = GraphDef::new("Reusable")
        .category("Test")
        .input(FuncInput::optional("value", DataType::Int))
        .output(FuncOutput::new("result", DataType::Int));
    let subgraph_json = serde_json::to_value(&subgraph)?;
    assert_eq!(subgraph_json["name"], "Reusable");
    assert_eq!(subgraph_json["category"], "Test");
    assert_eq!(subgraph_json["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(subgraph_json["outputs"].as_array().unwrap().len(), 1);
    assert!(subgraph_json.get("body").is_some());

    for format in SerdeFormat::all_formats_for_testing() {
        let bytes = serialize(&subgraph, format)?;
        let back: GraphDef = deserialize(&bytes, format)?;
        assert_eq!(subgraph, back, "{format:?} round-trips a definition whole");
    }

    Ok(())
}

#[test]
fn validate_rejects_node_ids_reused_across_graph_levels() {
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let node_id = NodeId::unique();
    let mut interior = GraphDef::new("duplicate id");
    interior.body.insert(node_id, node.clone());
    let graph_id = GraphId::unique();

    let mut graph = Graph::default();
    graph.insert(node_id, node);
    graph.insert_graph(graph_id, interior);

    let error = graph.validate().unwrap_err().to_string();
    assert!(error.contains("occurs in more than one authoring graph"));
}

#[test]
fn validate_rejects_graph_ids_reused_across_parents() {
    // The same def id planted under two parents — unreachable through
    // `clone_mapped` (it remaps graph ids at every copy boundary), so it's
    // corrupt input validation refuses: a bare graph id must be an
    // unambiguous document-wide address.
    let graph_id = GraphId::unique();
    let mut parent_a = GraphDef::new("parent a");
    parent_a.body.insert_graph(graph_id, GraphDef::new("dup"));
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, GraphDef::new("dup"));
    graph.insert_graph(GraphId::unique(), parent_a);

    let error = graph.validate().unwrap_err().to_string();
    assert!(
        error.contains("occurs in more than one parent graph"),
        "{error}"
    );
}

#[test]
fn insert_graph_replaces_existing_graph() {
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, GraphDef::new("original"));
    graph.insert_graph(graph_id, GraphDef::new("replacement"));
    assert_eq!(&graph.graphs[&graph_id].name, "replacement");
}

#[test]
fn validate_passes_for_valid_graph() {
    assert!(test_graph().validate().is_ok());
}

#[test]
fn validation_distinguishes_entry_graphs_from_subgraph_definitions() {
    // "A definition carries an interface, an entry graph doesn't" is a type
    // fact (`GraphDef` vs `Graph`), so only the *boundary-node* half of the
    // distinction is still checkable at runtime.
    let entry = Graph::default();
    assert!(entry.validate_with(&Library::default()).is_ok());

    let mut def = GraphDef::new("reusable")
        .input(FuncInput::optional("value", DataType::Int))
        .output(FuncOutput::new("result", DataType::Int));
    def.body.add(Node::new(NodeKind::GraphInput));
    def.body.add(Node::new(NodeKind::GraphOutput));
    assert_eq!(def.name, "reusable");
    assert_eq!(def.inputs.len(), 1);
    assert_eq!(def.outputs.len(), 1);
    assert!(def.body.validate().is_ok());

    // The boundary nodes that make it a definition are exactly what an
    // execution entry may not contain.
    let error = def.body.validate_with(&Default::default()).unwrap_err();
    assert!(matches!(error, GraphValidationError::EntryBoundaryNodes));
}

#[test]
fn validate_for_execution_validates_shared_graph_structure_and_recursion() {
    let graph_id = GraphId::unique();
    let mut shared = GraphDef::new("recursive");
    shared
        .body
        .add(Node::new(NodeKind::Graph(GraphLink::Shared(graph_id))));

    let mut library = Library::default();
    library.register_graph(graph_id, shared);

    let mut graph = Graph::default();
    graph.add(Node::new(NodeKind::Graph(GraphLink::Shared(graph_id))));

    let error = graph.validate_with(&library).unwrap_err().to_string();
    assert!(error.contains("recursive"));

    let graph_id = GraphId::unique();
    let mut shared = GraphDef::new("structurally invalid");
    shared.body.add(Node::new(NodeKind::GraphInput));
    shared.body.add(Node::new(NodeKind::GraphInput));

    let mut library = Library::default();
    library.register_graph(graph_id, shared);

    let mut graph = Graph::default();
    graph.add(Node::new(NodeKind::Graph(GraphLink::Shared(graph_id))));

    let error = graph.validate_with(&library).unwrap_err().to_string();
    assert!(error.contains("at most one GraphInput"));
}

/// An `entry_only` func is legal at the top level and nowhere below it: a
/// definition instanced twice runs its body twice, which is exactly what such a
/// func declares it cannot survive. Local and shared bodies are both rejected,
/// and `GraphDef::validate` rejects a body on its own — a definition is never an
/// entry however it is reached.
#[test]
fn entry_only_funcs_are_rejected_inside_every_definition_body() {
    let entry_only = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "watcher")
            .entry_only()
            .input(FuncInput::optional("v", DataType::Any)),
    );
    let ordinary = testing::with_stub_lambda(Func::new(FuncId::unique(), "plain"));
    let mut library = Library::default();
    library.add(entry_only.clone());
    library.add(ordinary.clone());

    // At the top level it validates like any other func.
    let mut graph = Graph::default();
    graph.add_func_node(&entry_only);
    assert!(graph.validate_with(&library).is_ok());

    // Inside a local definition — rejected, naming the offending node and func.
    let local_id = GraphId::unique();
    let mut local = GraphDef::new("local");
    let inner = local.body.add_func_node(&entry_only);
    let mut graph = Graph::default();
    graph.add_graph_node(&local, GraphLink::Local(local_id));
    graph.insert_graph(local_id, local);
    let error = graph.validate_with(&library).unwrap_err();
    let GraphValidationError::LocalGraph { name, source } = &error else {
        panic!("a local body reports through LocalGraph: {error:?}");
    };
    assert_eq!(name, "local");
    assert!(
        matches!(
            **source,
            GraphValidationError::EntryOnlyFunc { node_id, func_id }
                if node_id == inner && func_id == entry_only.id
        ),
        "it names the offending node and func: {source:?}"
    );

    // Inside a shared definition — same verdict through the other descent.
    let shared_id = GraphId::unique();
    let mut shared = GraphDef::new("shared");
    let inner = shared.body.add_func_node(&entry_only);
    let mut graph = Graph::default();
    graph.add_graph_node(&shared, GraphLink::Shared(shared_id));
    let mut library = library;
    library.register_graph(shared_id, shared);
    let error = graph.validate_with(&library).unwrap_err();
    let GraphValidationError::SharedGraph { name, source } = &error else {
        panic!("a shared body reports through SharedGraph: {error:?}");
    };
    assert_eq!(name, "shared");
    assert!(
        matches!(
            **source,
            GraphValidationError::EntryOnlyFunc { node_id, func_id }
                if node_id == inner && func_id == entry_only.id
        ),
        "the other descent reaches the same verdict: {source:?}"
    );

    // Library-gated, like `MissingFunc` and the const-only binding check: a
    // library-less validate cannot resolve the func, so it cannot see the flag.
    // Compilation always validates against one, so the invariant still holds
    // wherever a graph can actually run.
    let mut standalone = GraphDef::new("standalone");
    standalone.body.add_func_node(&entry_only);
    assert!(standalone.validate().is_ok());

    // An ordinary func in the same slot is fine, so the rule is the flag's
    // doing rather than "definitions reject funcs".
    let plain_id = GraphId::unique();
    let mut local = GraphDef::new("plain");
    local.body.add_func_node(&ordinary);
    let mut graph = Graph::default();
    graph.add_graph_node(&local, GraphLink::Local(plain_id));
    graph.insert_graph(plain_id, local);
    assert!(graph.validate_with(&library).is_ok());
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
        graph.find(id, NodeSearch::TopLevel).unwrap().cache,
        CacheMode::Both
    );

    // The func-less constructors have no func to copy from and seed `None`.
    assert_eq!(
        Node::new(NodeKind::Func(FuncId::unique())).cache,
        CacheMode::None
    );
}

#[test]
fn validate_rejects_dangling_binding() {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
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
    use crate::graph::node::definition::FuncId;
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
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::ExecutionBinding;
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
        let e_node = &compiled.program
            [compiled.program.e_node_index[&ExecutionNodeId::from_authoring(&[node])]];
        compiled.program.inputs[e_node.inputs.start as usize]
            .binding
            .clone()
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
fn validate_for_execution_tolerates_library_range_drift() {
    use crate::library::Library;

    let func = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "one_out").output(FuncOutput::new("o", DataType::Int)),
    );
    let mut library = Library::default();
    library.add(func.clone());

    let mut graph = Graph::default();
    let id = graph.add_func_node(&func);
    assert!(graph.validate_with(&library).is_ok());

    // Wiring the current library can't resolve — a binding, subscription, and
    // exposed event past the declared ranges — stays valid: drift is tolerated
    // (it degrades to unbound at flatten/plan time), never a compile error. See
    // `engine::tests::dangling_wiring_compiles_and_reports_missing_input`.
    graph.set_input_binding(InputPort::new(id, 5), Binding::bind(id, 7));
    graph.subscribe(id, 3, id);
    let mut child = GraphDef::new("child");
    let interior = child.body.add_func_node(&func);
    child.events.push(GraphEvent {
        name: "drifted".into(),
        emitter: interior,
        emitter_event_idx: 9,
    });
    graph.insert_graph(GraphId::unique(), child);
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

#[test]
fn validate_caps_graph_nesting_depth() {
    use crate::graph::MAX_NESTING_DEPTH;

    let nest = |levels: usize| {
        let mut graph = GraphDef::new("leaf");
        for _ in 0..levels {
            let mut parent = GraphDef::new("level");
            parent.body.insert_graph(GraphId::unique(), graph);
            graph = parent;
        }
        let mut root = Graph::default();
        root.insert_graph(GraphId::unique(), graph);
        root
    };

    // `nest(k)` puts the leaf definition at depth `k + 1`; the cap is the
    // parameter deciding, not the walk giving up.
    assert!(nest(MAX_NESTING_DEPTH - 2).validate().is_ok());
    let error = nest(MAX_NESTING_DEPTH).validate().unwrap_err();
    let mut source: &dyn std::error::Error = &error;
    while let Some(next) = source.source() {
        source = next;
    }
    assert_eq!(
        source.to_string(),
        format!("graph nesting exceeds {MAX_NESTING_DEPTH} levels")
    );
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
fn type_mismatched_wiring_flattens_as_unbound_through_wildcard_chains() {
    use crate::DataType;
    use crate::execution::compile::Compiler;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::ExecutionBinding;
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
        let e_node = &compiled.program
            [compiled.program.e_node_index[&ExecutionNodeId::from_authoring(&[sink])]];
        match &compiled.program.inputs[e_node.inputs.start as usize].binding {
            ExecutionBinding::Bind(addr) => {
                FlatSink::Bound(compiled.program.e_node_ids[addr.node_idx])
            }
            ExecutionBinding::Const(_) => FlatSink::Const,
            ExecutionBinding::None => FlatSink::Unbound,
        }
    };

    // The valid Float chain binds the sink to its passthrough producer
    // (passthroughs are real func nodes — only boundaries short-circuit).
    assert_eq!(
        sink_binding(&g),
        FlatSink::Bound(ExecutionNodeId::from_authoring(&[p2])),
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
        graph.resolve_output_type(&library, OutputPort::new(id, 0)),
        DataType::Any
    );
}

#[test]
fn input_type_resolves_declared_types_and_skips_boundaries() {
    use crate::DataType;
    use crate::library::Library;

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
    assert!(matches!(
        Graph::deserialize(&bytes, SerdeFormat::Bitcode),
        Err(GraphDeserializeError::InvalidGraph(_))
    ));

    let mut nil_key = Graph::default();
    nil_key
        .nodes
        .insert(NodeId::nil(), Node::new(NodeKind::Func(FuncId::unique())));
    let bytes = nil_key.serialize(SerdeFormat::Bitcode).unwrap();
    assert!(matches!(
        Graph::deserialize(&bytes, SerdeFormat::Bitcode),
        Err(GraphDeserializeError::InvalidGraph(_))
    ));

    // A definition's lineage is checked through its parent, since a def is
    // only ever decoded as part of the graph holding it.
    let mut nil_origin = Graph::default();
    nil_origin.insert_graph(
        GraphId::unique(),
        GraphDef::new("nil origin").origin(GraphId::nil()),
    );
    let bytes = nil_origin.serialize(SerdeFormat::Bitcode).unwrap();
    let error = Graph::deserialize(&bytes, SerdeFormat::Bitcode)
        .unwrap_err()
        .to_string();
    assert!(error.contains("graph has a nil origin"), "{error}");

    let mut duplicate_bindings = serde_json::to_value(test_graph()).unwrap();
    let bindings = duplicate_bindings["bindings"].as_array_mut().unwrap();
    bindings.push(bindings[0].clone());
    let bytes = serde_json::to_vec(&duplicate_bindings).unwrap();
    let decode_error = Graph::deserialize(&bytes, SerdeFormat::Json).unwrap_err();
    assert!(matches!(
        &decode_error,
        GraphDeserializeError::Deserialize(_)
    ));
    let error = decode_error.to_string();
    assert!(
        error.contains("duplicate binding for input port"),
        "{error}"
    );
}

#[test]
fn node_remove_test() -> TestResult {
    let mut graph = test_graph();

    let node_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    graph.find_mut(node_id, NodeSearch::TopLevel).unwrap().cache = CacheMode::Ram;
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

    graph.detach_node(node_id);

    assert!(graph.find_by_name("sum", NodeSearch::TopLevel).is_none());
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
fn only_boundary_kinds_are_boundaries() {
    let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
    assert!(!NodeKind::Func(func_id).is_boundary());
    assert!(!NodeKind::Graph(GraphLink::Local(GraphId::unique())).is_boundary());
    assert!(NodeKind::GraphInput.is_boundary());
    assert!(NodeKind::GraphOutput.is_boundary());
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
fn wiring_snapshot_round_trips_through_serde_and_restore() -> TestResult {
    let mut graph = test_graph();
    let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
    let get_a_id = graph
        .find_by_name("get_a", NodeSearch::TopLevel)
        .unwrap()
        .id;
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

    assert_eq!(
        graph.find(id, NodeSearch::TopLevel).unwrap().kind,
        NodeKind::Func(func.id)
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
    // Defaults are seeded at their *declared* port index, so gaps in the
    // interface don't shift the bindings that follow them.
    let graph_id = GraphId::unique();
    let def = GraphDef::new("Def").category("Test").inputs([
        FuncInput::optional("A", DataType::Int),
        FuncInput::optional("B", DataType::Int).default(3i64),
        FuncInput::optional("C", DataType::Int),
        FuncInput::optional("D", DataType::Int).default(5i64),
    ]);

    let mut graph = Graph::default();
    let id = graph.add_graph_node(&def, GraphLink::Local(graph_id));

    assert_eq!(
        graph.bindings.get(&InputPort::new(id, 1)),
        Some(&Binding::Const(3i64.into()))
    );
    assert_eq!(
        graph.bindings.get(&InputPort::new(id, 3)),
        Some(&Binding::Const(5i64.into()))
    );
    assert!(!graph.bindings.contains_key(&InputPort::new(id, 0)));
    assert!(!graph.bindings.contains_key(&InputPort::new(id, 2)));
}

#[test]
fn node_search_scope_gates_graph_interiors() {
    // A top-level node plus one two-levels-deep: a local graph whose
    // interior holds another local graph with the target node inside.
    let mut inner_graph = GraphDef::new("Inner");
    let mut deep = Node::new(NodeKind::Func(FuncId::unique()));
    deep.name = "deep".to_owned();
    let deep_id = inner_graph.body.add(deep);
    let inner_id = GraphId::unique();

    let mut outer_graph = GraphDef::new("Outer");
    outer_graph.body.insert_graph(inner_id, inner_graph);
    let outer_id = GraphId::unique();

    let mut graph = Graph::default();
    let mut top = Node::new(NodeKind::Func(FuncId::unique()));
    top.name = "top".to_owned();
    let top_id = graph.add(top);
    graph.insert_graph(outer_id, outer_graph);

    // Top-level node: found either way.
    assert!(graph.find(top_id, NodeSearch::TopLevel).is_some());
    assert!(graph.find(top_id, NodeSearch::Recursive).is_some());
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
    assert!(graph.find(deep_id, NodeSearch::TopLevel).is_none());
    assert!(graph.find(deep_id, NodeSearch::Recursive).is_some());
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
            .find(NodeId::unique(), NodeSearch::Recursive)
            .is_none()
    );
    assert!(
        graph
            .find_by_name("missing", NodeSearch::Recursive)
            .is_none()
    );

    graph.find_mut(deep_id, NodeSearch::Recursive).unwrap().name = "top".to_owned();
    assert_eq!(
        graph.find_by_name("top", NodeSearch::Recursive).unwrap().id,
        top_id
    );

    // The mutable lookup resolves identically and its edit lands on the
    // nested node.
    graph.find_mut(deep_id, NodeSearch::Recursive).unwrap().name = "renamed".to_owned();
    assert_eq!(
        graph.find(deep_id, NodeSearch::Recursive).unwrap().name,
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
    assert!(graph.find_mut(deep_id, NodeSearch::TopLevel).is_none());
}

#[test]
fn node_ports_resolve_every_kind_to_its_declaration() {
    // `Some(ports)` is an authoritative arity a caller may range-check
    // against; `None` means "unknowable here" and must *not* read as an empty
    // port list — the drift guards do `is_some_and(|p| idx >= p.len())`, so
    // the two decide opposite ways.
    let library = test_func_lib(TestFuncHooks::default());
    let mut def = GraphDef::new("S")
        .inputs([
            FuncInput::optional("a", DataType::Int),
            FuncInput::optional("b", DataType::Int),
        ])
        .output(FuncOutput::new("r", DataType::Int));
    let input = def.body.add(Node::new(NodeKind::GraphInput));
    let output = def.body.add(Node::new(NodeKind::GraphOutput));
    let body = &def.body;
    let node = |id: NodeId| body.find(id, NodeSearch::TopLevel).unwrap();

    // Both boundary nodes mirror the enclosing interface, which a bare body
    // doesn't carry — unknowable from here.
    assert!(body.node_ports(node(input), &library).is_none());
    assert!(body.node_ports(node(output), &library).is_none());

    // A graph *instance* reads its target's interface, even though the
    // instance's own body isn't involved.
    let mut root = Graph::default();
    let def_id = GraphId::unique();
    let instance_id = root.add_graph_node(&def, GraphLink::Local(def_id));
    root.insert_graph(def_id, def);
    let instance = root.find(instance_id, NodeSearch::TopLevel).unwrap();
    let ports = root.node_ports(instance, &library).unwrap();
    assert_eq!(ports.name, "S");
    assert_eq!(ports.inputs.len(), 2);
    assert_eq!(ports.outputs.len(), 1);
    assert_eq!(ports.events.len(), 0);
    assert!(
        ports.func.is_none(),
        "a composite has no func declaration to read flags from"
    );
    // …so the flags answer from the interface instead: this one relays an
    // output, and the two policy flags stay off until a compiled program can
    // speak for the interior.
    assert!(!ports.sink(), "a composite exposing an output is no sink");
    assert!(!ports.uncacheable());
    assert!(!ports.impure());

    // A func node resolves through the library to its own declaration.
    let sum = library.by_name("sum").unwrap();
    let sum_id = root.add_func_node(sum);
    let sum_node = root.find(sum_id, NodeSearch::TopLevel).unwrap();
    let ports = root.node_ports(sum_node, &library).unwrap();
    assert_eq!(ports.name, "sum");
    assert_eq!(ports.inputs.len(), sum.inputs.len());
    assert_eq!(ports.func.map(|func| func.id), Some(sum.id));

    // A func's flags come off its declaration verbatim — the same three
    // questions, answered from the other source. Each is set on one of the
    // two funcs below and clear on the other, so a flag wired to the wrong
    // field can't pass.
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

    // A composite with no outputs reads as a sink — the stand-in an editor
    // paints before the first compile.
    assert!(GraphDef::new("silent").ports().sink());

    // An unresolvable link is unknown, not empty — library drift must not
    // silently report every port out of range.
    let dangling = Node::new(NodeKind::Graph(GraphLink::Local(GraphId::unique())));
    assert!(root.node_ports(&dangling, &library).is_none());
    let missing_func = Node::new(NodeKind::Func(FuncId::unique()));
    assert!(root.node_ports(&missing_func, &library).is_none());
}

#[test]
fn node_events_expose_names_and_arity_for_both_declarations() {
    // `FuncEvent` and `GraphEvent` are different types; `NodeEvents` is the
    // common ground, so both spell arity and names the same way.
    let emitter = testing::with_stub_lambda(
        Func::new(FuncId::unique(), "ticker")
            .event("tick", EventLambda::default())
            .event("tock", EventLambda::default()),
    );
    let ports = emitter.ports();
    assert_eq!(ports.events.len(), 2);
    assert_eq!(ports.events.names().collect::<Vec<_>>(), ["tick", "tock"]);

    let mut def = GraphDef::new("D");
    let interior = def.body.add(Node::new(NodeKind::Func(emitter.id)));
    let def = def.event(GraphEvent {
        name: "exposed".into(),
        emitter: interior,
        emitter_event_idx: 0,
    });
    let ports = def.ports();
    assert_eq!(ports.events.len(), 1);
    assert_eq!(ports.events.names().collect::<Vec<_>>(), ["exposed"]);

    assert!(GraphDef::new("empty").ports().events.is_empty());
}

#[test]
fn resolve_graph_picks_local_or_linked_source() {
    let mut library = test_func_lib(TestFuncHooks::default());

    let linked_id = GraphId::unique();
    library.register_graph(linked_id, GraphDef::new("Linked").category("Test"));

    let mut graph = Graph::default();
    let local_id = GraphId::unique();
    graph.insert_graph(local_id, GraphDef::new("Local").category("Test"));

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

    // `resolve_graph` is parent-scoped by design; the recursive queries
    // reach any depth. Nest a def two levels down and address it bare.
    let deep_id = GraphId::unique();
    graph
        .graphs
        .get_mut(&local_id)
        .unwrap()
        .body
        .insert_graph(deep_id, GraphDef::new("Deep").category("Test"));
    assert!(
        graph
            .resolve_graph(GraphLink::Local(deep_id), &library)
            .is_none(),
        "parent-scoped resolution does not see nested defs"
    );
    assert_eq!(
        graph.find_graph(deep_id).unwrap().name,
        "Deep",
        "find_graph reaches a depth-2 def by bare id"
    );
    assert!(
        std::ptr::eq(
            graph.find_graph_parent(deep_id).unwrap(),
            &graph.find_graph(local_id).unwrap().body
        ),
        "the parent of the deep def is the mid-level graph's body"
    );
    assert!(
        std::ptr::eq(graph.find_graph_parent(local_id).unwrap(), &graph),
        "a top-level def's parent is the root itself"
    );
    graph.find_graph_mut(deep_id).unwrap().name = "Renamed".into();
    assert_eq!(
        graph.find_graph(deep_id).unwrap().name,
        "Renamed",
        "find_graph_mut writes through to the nested def"
    );
    assert!(graph.find_graph(GraphId::unique()).is_none());
}

// Subgraph-interface port edits below: `Graph::{detach,attach}_graph_{input,
// output}` and the renumbering they share. Each pair is an exact inverse, so
// the round-trip tests are the load-bearing ones.

fn int_input(name: &str) -> FuncInput {
    FuncInput::optional(name, DataType::Int)
}

fn int_output(name: &str) -> FuncOutput {
    FuncOutput::new(name, DataType::Int)
}

fn func_node() -> Node {
    Node::new(NodeKind::Func(FuncId::unique()))
}

fn const_int(value: i64) -> Binding {
    Binding::Const(StaticValue::Int(value))
}

#[derive(Debug)]
struct InputFixture {
    graph: Graph,
    graph_id: GraphId,
    boundary: NodeId,
    consumer: NodeId,
    instance_a: NodeId,
    instance_b: NodeId,
}

/// Child interface `[A, B, C]`; interior consumer reads all three boundary
/// outputs; pins on boundary outputs 1 and 2; instance A bound on all three
/// slots (10/11/12), instance B only on slot 1.
fn input_fixture() -> InputFixture {
    let mut child = GraphDef::new("child").inputs([int_input("A"), int_input("B"), int_input("C")]);
    let boundary = child.body.add(Node::new(NodeKind::GraphInput));
    let consumer = child.body.add(func_node());
    for idx in 0..3 {
        child
            .body
            .set_input_binding(InputPort::new(consumer, idx), Binding::bind(boundary, idx));
    }
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance_a = graph.add(Node::graph_instance(&child, GraphLink::Local(graph_id)));
    let instance_b = graph.add(Node::graph_instance(&child, GraphLink::Local(graph_id)));
    graph.insert_graph(graph_id, child);
    for (idx, value) in [10, 11, 12].into_iter().enumerate() {
        graph.set_input_binding(InputPort::new(instance_a, idx), const_int(value));
    }
    graph.set_input_binding(InputPort::new(instance_b, 1), const_int(21));
    InputFixture {
        graph,
        graph_id,
        boundary,
        consumer,
        instance_a,
        instance_b,
    }
}

#[test]
fn detach_and_attach_graph_input_round_trip() {
    let InputFixture {
        mut graph,
        graph_id,
        boundary,
        consumer,
        instance_a,
        instance_b,
    } = input_fixture();
    let original = graph.clone_verbatim();

    let snapshot = graph.snapshot_graph_input(graph_id, 1).unwrap();
    let detached = graph.detach_graph_input(graph_id, 1);
    assert_eq!(
        snapshot, detached,
        "snapshot is exactly what detach removes"
    );

    assert_eq!(detached.spec.name, "B");
    assert_eq!(
        detached.interior,
        vec![BindingEntry {
            port: InputPort::new(consumer, 1),
            binding: Binding::bind(boundary, 1),
        }]
    );
    // Both instances lose their slot-1 binding: A's 11 and B's 21.
    assert_eq!(detached.parent.len(), 2);
    assert!(
        detached
            .parent
            .iter()
            .any(|entry| entry.port == InputPort::new(instance_a, 1)
                && entry.binding == const_int(11))
    );
    assert!(
        detached
            .parent
            .iter()
            .any(|entry| entry.port == InputPort::new(instance_b, 1)
                && entry.binding == const_int(21))
    );

    // Interface compacts [A, B, C] -> [A, C].
    let child = graph.graphs.get(&graph_id).unwrap();
    let names: Vec<&str> = child
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    assert_eq!(names, ["A", "C"]);
    // Interior: in0 keeps slot 0, in1's edge was severed, in2's source
    // shifted 2 -> 1.
    assert_eq!(
        child.body.bindings.get(&InputPort::new(consumer, 0)),
        Some(&Binding::bind(boundary, 0))
    );
    assert_eq!(child.body.bindings.get(&InputPort::new(consumer, 1)), None);
    assert_eq!(
        child.body.bindings.get(&InputPort::new(consumer, 2)),
        Some(&Binding::bind(boundary, 1))
    );
    // Instance A: 0 stays 10, old 2 (12) shifted to 1, slot 2 cleared;
    // instance B: fully unbound.
    assert_eq!(
        graph.bindings.get(&InputPort::new(instance_a, 0)),
        Some(&const_int(10))
    );
    assert_eq!(
        graph.bindings.get(&InputPort::new(instance_a, 1)),
        Some(&const_int(12))
    );
    assert_eq!(graph.bindings.get(&InputPort::new(instance_a, 2)), None);
    assert!(
        !graph.bindings.keys().any(|port| port.node_id == instance_b),
        "instance B's only binding was on the removed slot"
    );

    graph.attach_graph_input(graph_id, detached);
    assert_eq!(
        graph, original,
        "attach restores the exact pre-detach graph"
    );
}

#[test]
fn detach_graph_input_at_each_index_severs_that_slot() {
    // Parameterized: removing slot 0 vs slot 2 must produce different
    // interfaces and remaps.
    for (idx, expect_names, expect_a) in [
        (0usize, ["B", "C"], [11, 12]),
        (2usize, ["A", "B"], [10, 11]),
    ] {
        let fixture = input_fixture();
        let mut graph = fixture.graph;
        graph.detach_graph_input(fixture.graph_id, idx);
        let child = graph.graphs.get(&fixture.graph_id).unwrap();
        let names: Vec<&str> = child
            .inputs
            .iter()
            .map(|input| input.name.as_str())
            .collect();
        assert_eq!(names, expect_names, "detach idx {idx}");
        for (slot, value) in expect_a.into_iter().enumerate() {
            assert_eq!(
                graph
                    .bindings
                    .get(&InputPort::new(fixture.instance_a, slot)),
                Some(&const_int(value)),
                "detach idx {idx}, instance slot {slot}"
            );
        }
        assert_eq!(
            graph.bindings.get(&InputPort::new(fixture.instance_a, 2)),
            None,
            "detach idx {idx} leaves two instance bindings"
        );
    }
}

#[derive(Debug)]
struct OutputFixture {
    graph: Graph,
    graph_id: GraphId,
    boundary: NodeId,
    producer: NodeId,
    instance: NodeId,
    consumer_a: NodeId,
    consumer_b: NodeId,
}

/// Child interface outputs `[X, Y, Z]` fed by an interior producer; parent
/// consumers read instance outputs 1 and 2, with pins on both.
fn output_fixture() -> OutputFixture {
    let mut child =
        GraphDef::new("child").outputs([int_output("X"), int_output("Y"), int_output("Z")]);
    let boundary = child.body.add(Node::new(NodeKind::GraphOutput));
    let producer = child.body.add(func_node());
    child
        .body
        .set_input_binding(InputPort::new(boundary, 0), Binding::bind(producer, 0));
    child
        .body
        .set_input_binding(InputPort::new(boundary, 1), Binding::bind(producer, 0));
    child
        .body
        .set_input_binding(InputPort::new(boundary, 2), Binding::bind(producer, 1));

    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance = graph.add(Node::graph_instance(&child, GraphLink::Local(graph_id)));
    let consumer_a = graph.add(func_node());
    let consumer_b = graph.add(func_node());
    graph.insert_graph(graph_id, child);
    graph.set_input_binding(InputPort::new(consumer_a, 0), Binding::bind(instance, 1));
    graph.set_input_binding(InputPort::new(consumer_b, 0), Binding::bind(instance, 2));
    OutputFixture {
        graph,
        graph_id,
        boundary,
        producer,
        instance,
        consumer_a,
        consumer_b,
    }
}

#[test]
fn detach_and_attach_graph_output_round_trip() {
    let OutputFixture {
        mut graph,
        graph_id,
        boundary,
        producer,
        instance,
        consumer_a,
        consumer_b,
    } = output_fixture();
    let original = graph.clone_verbatim();

    let snapshot = graph.snapshot_graph_output(graph_id, 1).unwrap();
    let detached = graph.detach_graph_output(graph_id, 1);
    assert_eq!(
        snapshot, detached,
        "snapshot is exactly what detach removes"
    );

    assert_eq!(detached.spec.name, "Y");
    assert_eq!(
        detached.interior,
        vec![BindingEntry {
            port: InputPort::new(boundary, 1),
            binding: Binding::bind(producer, 0),
        }]
    );
    assert_eq!(
        detached.parent,
        vec![BindingEntry {
            port: InputPort::new(consumer_a, 0),
            binding: Binding::bind(instance, 1),
        }]
    );

    // Interface [X, Y, Z] -> [X, Z].
    let child = graph.graphs.get(&graph_id).unwrap();
    let names: Vec<&str> = child
        .outputs
        .iter()
        .map(|output| output.name.as_str())
        .collect();
    assert_eq!(names, ["X", "Z"]);
    // Interior: slot 1's binding removed, slot 2's rekeyed to 1.
    assert_eq!(
        child.body.bindings.get(&InputPort::new(boundary, 0)),
        Some(&Binding::bind(producer, 0))
    );
    assert_eq!(
        child.body.bindings.get(&InputPort::new(boundary, 1)),
        Some(&Binding::bind(producer, 1))
    );
    assert_eq!(child.body.bindings.get(&InputPort::new(boundary, 2)), None);
    // Parent: consumer A severed, consumer B's source shifted 2 -> 1,
    // pin 1 dropped and pin 2 shifted to 1.
    assert_eq!(graph.bindings.get(&InputPort::new(consumer_a, 0)), None);
    assert_eq!(
        graph.bindings.get(&InputPort::new(consumer_b, 0)),
        Some(&Binding::bind(instance, 1))
    );
    graph.attach_graph_output(graph_id, detached);
    assert_eq!(
        graph, original,
        "attach restores the exact pre-detach graph"
    );
}

#[test]
#[should_panic(expected = "does not sit on the detached input slot")]
fn attach_rejects_an_instance_binding_off_its_slot() {
    let fixture = input_fixture();
    let mut graph = fixture.graph;
    let mut detached = graph.detach_graph_input(fixture.graph_id, 1);
    detached.parent[0].port.port_idx = 0;
    graph.attach_graph_input(fixture.graph_id, detached);
}

#[test]
fn a_rejected_attach_leaves_the_graph_untouched() {
    // Every record check runs before the first mutation, so a malformed
    // record can't half-apply and strand the interface mid-shift.
    let fixture = input_fixture();
    let mut graph = fixture.graph;
    let mut detached = graph.detach_graph_input(fixture.graph_id, 1);
    let after_detach = graph.clone_verbatim();
    detached.parent[0].port.port_idx = 0;

    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        graph.attach_graph_input(fixture.graph_id, detached);
    }));
    assert!(refused.is_err(), "a malformed record must be refused");
    assert_eq!(graph, after_detach, "refused before touching the graph");
}

/// The severed interior edge's port was re-bound in the meantime; the
/// shift can't vacate it (it renumbers boundary-fed *values*, not this
/// consumer-keyed port), so restoring would destroy an authored wire.
///
/// **Refusing is only half the contract — the newer wire has to survive
/// it.** Restoring first and asserting afterwards meant the overwrite had
/// already happened when the panic fired: the `Const(99)` authored after
/// detachment was gone, the entries restored ahead of it stayed applied,
/// and the parent-side slots had already shifted. A caller that caught
/// the panic — the editor's undo replay does exactly this — kept a graph
/// that had been half-attached and had silently lost a binding.
#[test]
fn attach_refusing_an_overlapping_binding_leaves_the_graph_untouched() {
    let fixture = input_fixture();
    let mut graph = fixture.graph;
    let detached = graph.detach_graph_input(fixture.graph_id, 1);
    let child = graph.graphs.get_mut(&fixture.graph_id).unwrap();
    let overlapping = InputPort::new(fixture.consumer, 1);
    child.body.set_input_binding(overlapping, const_int(99));
    let before_attach = graph.clone_verbatim();

    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        graph.attach_graph_input(fixture.graph_id, detached);
    }));
    let message = *refused
        .expect_err("an overlapping binding must be refused")
        .downcast::<String>()
        .expect("assert! panics carry a String");
    assert!(
        message.contains("created after detachment"),
        "unexpected panic: {message}",
    );

    assert_eq!(
        graph.graphs[&fixture.graph_id]
            .body
            .bindings
            .get(&overlapping),
        Some(&const_int(99)),
        "the binding authored after detachment must survive the refusal",
    );
    assert_eq!(
        graph, before_attach,
        "a refused attach must not shift slots or restore any entry",
    );
}

#[test]
#[should_panic(expected = "does not read the detached output slot")]
fn attach_rejects_a_consumer_binding_off_its_slot() {
    let fixture = output_fixture();
    let mut graph = fixture.graph;
    let mut detached = graph.detach_graph_output(fixture.graph_id, 1);
    detached.parent[0].binding = Binding::bind(fixture.instance, 0);
    graph.attach_graph_output(fixture.graph_id, detached);
}

#[test]
fn snapshot_returns_none_for_missing_graph_or_slot() {
    let fixture = input_fixture();
    assert!(
        fixture
            .graph
            .snapshot_graph_input(GraphId::unique(), 0)
            .is_none(),
        "unknown graph id"
    );
    assert!(
        fixture
            .graph
            .snapshot_graph_input(fixture.graph_id, 3)
            .is_none(),
        "index past the interface"
    );
    assert!(
        fixture
            .graph
            .snapshot_graph_output(fixture.graph_id, 0)
            .is_none(),
        "no authored outputs on the input fixture"
    );
}

#[test]
fn detach_without_boundary_node_still_removes_spec_and_instance_bindings() {
    // A child that declares an interface but has no GraphInput node —
    // detach drops the spec and the instance wiring; there is no interior
    // to touch.
    let child = GraphDef::new("bare").inputs([int_input("A"), int_input("B")]);
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance = graph.add(Node::graph_instance(&child, GraphLink::Local(graph_id)));
    graph.insert_graph(graph_id, child);
    graph.set_input_binding(InputPort::new(instance, 0), const_int(1));
    graph.set_input_binding(InputPort::new(instance, 1), const_int(2));
    let original = graph.clone_verbatim();

    let detached = graph.detach_graph_input(graph_id, 0);
    assert!(detached.interior.is_empty());
    assert_eq!(detached.parent.len(), 1);
    let child = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(child.inputs[0].name, "B");
    assert_eq!(
        graph.bindings.get(&InputPort::new(instance, 0)),
        Some(&const_int(2)),
        "slot 1 shifted down"
    );

    graph.attach_graph_input(graph_id, detached);
    assert_eq!(graph, original);
}
