use super::*;
use crate::execution::compile::error::CompiledGraphValidationError;
use crate::execution::compiled::{CompiledGraph, ExecutionBinding};
use crate::execution::identity::NodeIdx;
use crate::execution::identity::OutputAddr;
use crate::execution::lower::lowered_graph::internals::LoweredGraphBuilder;
use crate::graph::Binding;
use crate::graph::func::Func;
use crate::graph::func::event::EventLambda;
use crate::graph::identity::{FuncId, InputPort, NodeId};
use crate::testing::{self, TestFuncHooks, test_func_lib, test_graph};

/// Event edges get the same treatment as bind fixups: an endpoint lowering
/// never emitted is a lowering bug, so wiring panics instead of dropping the
/// edge, and the compiled artifact still carries a range backstop.
#[test]
fn subscription_wiring_rejects_an_endpoint_outside_the_program() {
    let mut library = test_func_lib(TestFuncHooks::default());
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "ticker")
            .category("Test")
            .sink()
            .event("tick", EventLambda::default()),
    ));
    let mut graph = Graph::default();
    let emitter = graph.add(library.by_name("ticker").unwrap().into());
    let subscriber = graph.add(library.by_name("Print").unwrap().into());
    graph.subscribe(emitter, 0, subscriber);

    let mut compiled = Compiler::default().compile(&graph, &library).unwrap();
    let emitter_idx = compiled.node_index[&emitter];
    let events = compiled[emitter_idx].events;
    assert_eq!(
        compiled.events[events][0].subscribers.len(),
        1,
        "the authored subscription wired one subscriber"
    );

    // The artifact check catches a subscriber index that names no node.
    // (Wiring one the walk never emitted panics at link — covered there.)
    let past_the_end = NodeIdx(compiled.e_nodes.len() as u32);
    compiled.events[events][0].subscribers[0] = past_the_end;
    assert!(
        matches!(
            validate::validate(&compiled, &library),
            Err(CompiledGraphValidationError::MissingEventSubscriber { subscriber, .. })
                if subscriber == past_the_end
        ),
        "a subscriber past the node vector is caught by validation"
    );
}

/// The binding-integrity backstops. `intern_bindings` mints an address only
/// from a successful id lookup and a real compile can't reach either arm, so
/// they are only worth keeping if they still fire — corrupt a compiled
/// program's interned address two ways and check both do.
#[test]
fn validation_rejects_a_binding_that_does_not_name_a_real_output() {
    let library = test_func_lib(TestFuncHooks::default());
    let compile = || {
        Compiler::default()
            .compile(&test_graph(), &library)
            .unwrap()
    };
    let bound_input = |compiled: &CompiledGraph| {
        compiled
            .inputs
            .iter_indexed()
            .find(|(_, input)| matches!(input.binding, ExecutionBinding::Bind(_)))
            .map(|(input_idx, _)| input_idx)
            .expect("the test graph wires bindings")
    };

    // One past the last node: the address names no node at all.
    let mut compiled = compile();
    let past_the_end = NodeIdx(compiled.e_nodes.len() as u32);
    let input = bound_input(&compiled);
    compiled.inputs[input].binding = ExecutionBinding::Bind(OutputAddr {
        node_idx: past_the_end,
        port_idx: 0,
    });
    assert!(
        matches!(
            validate::validate(&compiled, &library),
            Err(CompiledGraphValidationError::MissingBindingTarget { target, .. })
                if target.node_idx == past_the_end
        ),
        "a bind past the node vector is a missing target"
    );

    // A real node, one past its last output port.
    let mut compiled = compile();
    let input = bound_input(&compiled);
    let ExecutionBinding::Bind(address) = compiled.inputs[input].binding else {
        unreachable!("bound_input selected a bind")
    };
    let port_idx = compiled[address.node_idx].outputs.len;
    compiled.inputs[input].binding = ExecutionBinding::Bind(OutputAddr {
        node_idx: address.node_idx,
        port_idx,
    });
    assert!(
        matches!(
            validate::validate(&compiled, &library),
            Err(CompiledGraphValidationError::BindingOutputOutOfRange { target, .. })
                if target.port_idx == port_idx
        ),
        "a bind past the producer's last port is out of range"
    );
}

/// One sort settles the program's dense node order. Ids are uuids, so the order
/// nodes are authored in says nothing about the order they are adopted in — the
/// id column is the authored set, sorted, and nothing else.
#[test]
fn dense_order_is_id_order() {
    let library = test_func_lib(TestFuncHooks::default());
    let get_b = library.by_name("get_b").unwrap();
    let mut graph = Graph::default();
    let authored: Vec<NodeId> = (0..8).map(|_| graph.add(get_b.into())).collect();

    let compiled = Compiler::default().compile(&graph, &library).unwrap();

    let node_ids: Vec<_> = compiled.node_ids.iter().copied().collect();
    let mut expected = authored.clone();
    expected.sort();
    assert_eq!(node_ids, expected, "nodes are adopted in id order");
    for node_id in authored {
        assert!(compiled.contains(node_id));
    }
}

#[test]
fn validation_returns_compiled_mismatches() {
    let node_id = NodeId::unique();
    let missing_func = FuncId::unique();
    let mut builder = LoweredGraphBuilder::default();
    builder.insert_node(node_id);
    let mut lowered = builder.build();
    lowered.e_nodes[0].func_id = missing_func;
    let mut compiled = CompiledGraph::default();
    link::Linker::default().link(&lowered, &Library::default(), &mut compiled);

    assert_eq!(
        validate::validate(&compiled, &Library::default())
            .unwrap_err()
            .to_string(),
        format!("execution node {node_id:?} references missing func {missing_func:?}")
    );
    assert!(compiled.contains(node_id));
    assert!(!compiled.contains(NodeId::unique()));
}

/// Everything one compile observably produced, in a deterministic order and in
/// the **stable id space** — bind targets and subscribers by id rather than
/// index, so two artifacts can't match by an index coincidence.
fn summary(compiled: &CompiledGraph, authored: &[NodeId]) -> Vec<String> {
    let program = &compiled;
    let mut out = Vec::new();
    for (node_idx, e_node) in program.e_nodes.iter_indexed() {
        let node_id = program.node_ids[node_idx];
        out.push(format!(
            "node {node_id:?} func={:?} sink={} disabled={} cache={:?} special={:?}",
            e_node.func_id, e_node.sink, e_node.disabled, e_node.cache, e_node.special,
        ));
        for input in &program.inputs[e_node.inputs] {
            let binding = match &input.binding {
                ExecutionBinding::None => "none".to_string(),
                ExecutionBinding::Const(value) => format!("const {value:?}"),
                ExecutionBinding::Bind(address) => format!(
                    "bind {:?}#{}",
                    program.node_ids[address.node_idx], address.port_idx
                ),
            };
            out.push(format!(
                "  in required={} fs_path={} {binding}",
                input.required, input.stamps_fs_path
            ));
        }
        for output in &program.outputs[e_node.outputs] {
            out.push(format!("  out {output:?}"));
        }
        for event in &program.events[e_node.events] {
            let subscribers: Vec<_> = event
                .subscribers
                .iter()
                .map(|&idx| program.node_ids[idx])
                .collect();
            out.push(format!("  event subscribers={subscribers:?}"));
        }
    }
    // The host-facing index, which is built from its own scratch.
    for &node_id in authored {
        out.push(format!(
            "authored {node_id:?} compiled={}",
            compiled.contains(node_id),
        ));
    }
    out
}

/// A sink reading a source, plus an unwired node — enough for every question
/// the artifact answers about an authored node to have a distinct answer.
struct Fixture {
    library: Library,
    graph: Graph,
    source: NodeId,
    sink: NodeId,
    loose: NodeId,
}

fn fixture() -> Fixture {
    let library = test_func_lib(TestFuncHooks::default());
    let mut graph = Graph::default();
    let source = graph.add_func_node(library.by_name("get_a").unwrap());
    let sink = graph.add_func_node(library.by_name("Print").unwrap());
    let loose = graph.add_func_node(library.by_name("get_b").unwrap());
    graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(source, 0));
    Fixture {
        library,
        graph,
        source,
        sink,
        loose,
    }
}

/// Every authored node holds compiled work, whatever its role — a source, its
/// reader, and a node nothing wires to alike. An id the program never held does
/// not.
#[test]
fn the_artifact_answers_for_each_authored_node() {
    let f = fixture();
    let compiled = Compiler::default().compile(&f.graph, &f.library).unwrap();

    for node_id in [f.source, f.sink, f.loose] {
        assert!(compiled.contains(node_id));
    }
    assert!(!compiled.contains(NodeId::unique()));
}

/// Evicting a node reaches everything downstream of it, reflexively — and stops
/// at a node nothing connects to.
#[test]
fn the_consumer_closure_reaches_downstream_and_stops() {
    let f = fixture();
    let compiled = Compiler::default().compile(&f.graph, &f.library).unwrap();

    let mut from_source = compiled.data_consumer_closure(&[f.source]);
    from_source.sort();
    let mut expected = vec![f.source, f.sink];
    expected.sort();
    assert_eq!(from_source, expected, "the source reaches its reader");

    assert_eq!(
        compiled.data_consumer_closure(&[f.sink]),
        vec![f.sink],
        "a terminal node reaches only itself"
    );
    assert_eq!(
        compiled.data_consumer_closure(&[f.loose]),
        vec![f.loose],
        "an unwired node reaches only itself"
    );
    assert!(
        compiled
            .data_consumer_closure(&[NodeId::unique()])
            .is_empty()
    );
}

/// Every buffer a compile fills is owned by the `Compiler` and reused, so the
/// hazard the reuse introduces is one compile's leftovers reaching the next.
/// Compiling two graphs that share no node must leave the second artifact
/// identical to what a `Compiler` that had never run produces for it.
#[test]
fn a_reused_compiler_produces_what_a_fresh_one_does() {
    let first = fixture();
    let second = fixture();
    let authored = [second.source, second.sink, second.loose];

    let mut reused = Compiler::default();
    reused.compile(&first.graph, &first.library).unwrap();
    let after_reuse = reused.compile(&second.graph, &second.library).unwrap();
    let fresh = Compiler::default()
        .compile(&second.graph, &second.library)
        .unwrap();

    assert_eq!(
        summary(&after_reuse, &authored),
        summary(&fresh, &authored),
        "a compile carried something over from the one before it"
    );
    // The summary is only worth comparing if it saw the whole artifact.
    assert_eq!(after_reuse.e_nodes.len(), 3);
}

/// The same graph twice through one `Compiler`: the reused buffers must not
/// accumulate, which a growing pool would show as duplicated ports or nodes.
#[test]
fn recompiling_one_graph_does_not_accumulate_in_the_reused_buffers() {
    let f = fixture();
    let authored = [f.source, f.sink, f.loose];
    let mut compiler = Compiler::default();

    let first = compiler.compile(&f.graph, &f.library).unwrap();
    let first_summary = summary(&first, &authored);
    let second = compiler.compile(&f.graph, &f.library).unwrap();

    assert_eq!(summary(&second, &authored), first_summary);
    assert_eq!(second.inputs.len(), first.inputs.len());
    assert_eq!(second.outputs.len(), first.outputs.len());
}
