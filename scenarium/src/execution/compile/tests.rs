use super::*;
use crate::common::column::Idx;
use crate::execution::compile::error::CompiledGraphValidationError;
use crate::execution::compiled::ExecutionNode;
use crate::execution::compiled::{CompiledGraph, ExecutionBinding};
use crate::execution::identity::NodeIdx;
use crate::execution::identity::OutputAddr;
use crate::graph::Binding;
use crate::graph::func::event::EventLambda;
use crate::graph::func::{Func, FuncInput, FuncOutput};
use crate::graph::identity::{FuncId, InputPort, NodeId};
use crate::testing::{self, TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, StaticValue};

/// The walk wires a subscription to the emitter's own event slot, and the
/// artifact carries a range backstop under it — kept because the subscriber
/// index the walk writes is the one thing a run dereferences without checking.
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

    // The artifact check catches a subscriber index that names no node. The
    // walk cannot mint one — every index comes out of the placement, which
    // covers exactly the graph's nodes — so it is corrupted here by hand.
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

/// The binding-integrity backstops. The walk mints an address only from the
/// placement, so a real compile can't reach either arm — they are worth keeping
/// only if they still fire, so corrupt a compiled program's interned address two
/// ways and check both do.
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
    let mut compiled = CompiledGraph::default();
    compiled.push(
        node_id,
        ExecutionNode {
            func_id: missing_func,
            ..Default::default()
        },
    );

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

const PRODUCER: u128 = 1;
const CONSUMER: u128 = 2;

/// A producer declaring one output of `out_ty`, and a consumer declaring one
/// required input of `in_ty`. The two funcs every test below wires together.
fn library(out_ty: DataType, in_ty: DataType) -> Library {
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::from_u128(PRODUCER), "producer").output(FuncOutput::new("out", out_ty)),
    ));
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::from_u128(CONSUMER), "consumer").input(FuncInput::required("in", in_ty)),
    ));
    library
}

fn lower(graph: &Graph, library: &Library) -> CompiledGraph {
    Compiler::default()
        .compile(graph, library)
        .expect("the walk fixtures compile")
}

/// A producer and a consumer, optionally wired, lowered. Returns the program
/// alongside both node ids so a test can name either end.
fn wired(library: &Library, binding: Option<Binding>) -> (CompiledGraph, NodeId, NodeId) {
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let consumer = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    if let Some(binding) = binding {
        graph.set_input_binding(InputPort::new(consumer, 0), binding);
    }
    (lower(&graph, library), producer, consumer)
}

/// Every authored node becomes exactly one execution node carrying its own id,
/// and the dense order is the id order — `Graph::iter` is a `HashMap` walk, so
/// the order nodes are reached in must not reach the artifact.
#[test]
fn places_one_node_per_authored_node_in_id_order() {
    let library = library(DataType::Int, DataType::Int);
    let (program, producer, consumer) = wired(&library, None);

    assert_eq!(program.e_nodes.len(), 2);
    let mut expected = vec![producer, consumer];
    expected.sort();
    assert_eq!(
        program.node_ids.iter().copied().collect::<Vec<_>>(),
        expected
    );
    for (position, node_id) in expected.iter().enumerate() {
        assert_eq!(program.node_index[node_id], NodeIdx(position as u32));
    }
}

/// A node's port runs come from its func's arity and are packed in placement
/// order, so a node owns exactly the run its own declaration claimed.
#[test]
fn packs_one_port_run_per_node_from_its_declaration() {
    let library = library(DataType::Int, DataType::Int);
    let (program, producer, consumer) = wired(&library, None);

    let producer_node = program.by_id(producer);
    let consumer_node = program.by_id(consumer);
    assert_eq!(
        (producer_node.outputs.len, producer_node.inputs.len),
        (1, 0)
    );
    assert_eq!(
        (consumer_node.outputs.len, consumer_node.inputs.len),
        (0, 1)
    );
    assert_eq!(
        program.outputs.len(),
        1,
        "one output port across both nodes, typed from the producer's declaration"
    );
    assert_eq!(program.outputs[producer_node.outputs][0], DataType::Int);
    assert_eq!(program.inputs.len(), 1);
    assert_eq!(
        producer_node.outputs.start, 0,
        "the only run starts the pool"
    );
}

/// A wire is interned to the producer's dense address as the walk resolves it —
/// the right node index and the port index the authored wire named.
#[test]
fn interns_a_wire_to_the_producers_dense_address() {
    let library = library(DataType::Int, DataType::Int);
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let consumer = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    let program = lower(&graph, &library);

    let input = &program.inputs[program.by_id(consumer).inputs][0];
    let ExecutionBinding::Bind(address) = input.binding else {
        panic!("expected an interned bind, got {:?}", input.binding);
    };
    assert_eq!(
        address,
        OutputAddr {
            node_idx: program.node_index[&producer],
            port_idx: 0,
        },
    );
    // …and that address resolves into the producer's own output run, wherever
    // the id sort placed it.
    assert_eq!(
        program.output_idx(address).idx(),
        program.by_id(producer).outputs.start as usize,
    );
    assert!(
        input.required,
        "the declaration's flag travels with the port"
    );
}

/// The type gate: a wire whose producer no longer fits the consumer lowers as
/// unbound rather than severing the authored wiring, so it revives when the
/// types line up again.
#[test]
fn drops_a_type_mismatched_wire_to_unbound() {
    let library = library(DataType::String, DataType::Int);
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let consumer = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    let program = lower(&graph, &library);

    let consumer_node = program.by_id(consumer);
    assert!(
        matches!(
            program.inputs[consumer_node.inputs][0].binding,
            ExecutionBinding::None
        ),
        "a String producer does not satisfy an Int input"
    );
    assert!(
        graph.bindings.contains_key(&InputPort::new(consumer, 0)),
        "the authored wire itself is untouched"
    );
}

/// A const that satisfies its input travels through verbatim; a mismatched one
/// lowers unbound, on the same terms as a wire.
#[test]
fn keeps_a_satisfying_const_and_drops_a_mismatched_one() {
    let library = library(DataType::Int, DataType::Int);
    for (value, kept) in [
        (StaticValue::Int(7), true),
        (StaticValue::String("no".to_owned()), false),
    ] {
        let (program, _, consumer) = wired(&library, Some(Binding::Const(value)));
        let consumer_node = program.by_id(consumer);
        assert_eq!(
            matches!(
                program.inputs[consumer_node.inputs][0].binding,
                ExecutionBinding::Const(_)
            ),
            kept,
        );
    }
}

/// A disabled node keeps its place in the program — planning excludes it, the
/// walk does not — and the flag rides along for the planner to read.
#[test]
fn carries_the_disabled_flag_without_dropping_the_node() {
    let library = library(DataType::Int, DataType::Int);
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    graph.find_mut(producer).unwrap().disabled = true;
    let program = lower(&graph, &library);

    assert_eq!(program.e_nodes.len(), 1);
    assert!(program.by_id(producer).disabled);
}

/// The walk keeps only scratch, so one `Compiler` serves every compile and a
/// second walk cannot observe the first.
#[test]
fn a_second_walk_cannot_observe_the_first() {
    let library = library(DataType::Int, DataType::Int);
    let mut graph = Graph::default();
    graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let mut compiler = Compiler::default();
    compiler.compile(&graph, &library).unwrap();
    let program = compiler.compile(&graph, &library).unwrap();

    assert_eq!(program.e_nodes.len(), 1);
    assert_eq!(
        program.outputs.len(),
        1,
        "the port pools restart from empty"
    );
    assert!(program.events.is_empty());
}

/// The walk stamps each output with the type it *resolved*, wildcards followed —
/// so the program carries a type no declaration in the library mentions, rather
/// than the `Any` a re-derivation off the declaration alone would produce.
#[test]
fn stamps_each_output_with_the_resolved_type() {
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::from_u128(PRODUCER), "producer")
            .output(FuncOutput::new("out", DataType::String)),
    ));
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::from_u128(CONSUMER), "passthrough")
            .input(FuncInput::required("mirrored", DataType::Any))
            .wildcard_output("out", 0),
    ));
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let pass = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    graph.set_input_binding(InputPort::new(pass, 0), Binding::bind(producer, 0));

    let program = lower(&graph, &library);
    let pass_node = program.by_id(pass);
    assert_eq!(
        program.outputs[pass_node.outputs],
        [DataType::String],
        "the wildcard followed its mirror to the producer's String"
    );
}

/// Each event port gets its declared lambda and exactly the subscribers this
/// graph wires to it, so an empty list means "nothing subscribes" rather than
/// "not wired yet" — including for an emitter placed after its subscriber.
#[test]
fn wires_each_event_with_the_subscribers_resolved_for_it() {
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::from_u128(PRODUCER), "emitter")
            .event("quiet", EventLambda::default())
            .event("subscribed", EventLambda::default()),
    ));
    library.add(testing::with_stub_lambda(Func::new(
        FuncId::from_u128(CONSUMER),
        "listener",
    )));
    let mut graph = Graph::default();
    let emitter = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let subscriber = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    graph.subscribe(emitter, 1, subscriber);

    let program = lower(&graph, &library);
    let emitter_node = program.by_id(emitter);
    assert_eq!(emitter_node.events.len, 2);
    let subscribers: Vec<_> = program.events[emitter_node.events]
        .iter()
        .map(|event| event.subscribers.clone())
        .collect();
    assert_eq!(
        subscribers,
        vec![vec![], vec![program.node_index[&subscriber]]],
        "only the subscribed port carries a subscriber"
    );
}

/// A subscription whose emitter or subscriber is disabled wires nothing, and
/// neither does one naming an event the func no longer declares — the same
/// drift tolerance the type gate applies to data edges.
#[test]
fn drops_subscriptions_that_cannot_fire() {
    let emitter_func = |events: usize| {
        let mut func = Func::new(FuncId::from_u128(PRODUCER), "emitter");
        for i in 0..events {
            func = func.event(format!("e{i}"), EventLambda::default());
        }
        testing::with_stub_lambda(func)
    };
    let build = |events: usize, disable: bool| {
        let mut library = Library::default();
        library.add(emitter_func(events));
        library.add(testing::with_stub_lambda(Func::new(
            FuncId::from_u128(CONSUMER),
            "listener",
        )));
        let mut graph = Graph::default();
        let emitter = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
        let subscriber = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
        // Authored against a two-event declaration; `events == 1` is the library
        // having since dropped the port this names.
        graph.subscribe(emitter, 1, subscriber);
        if disable {
            graph.find_mut(subscriber).unwrap().disabled = true;
        }
        let program = lower(&graph, &library);
        let events = program.by_id(emitter).events;
        program.events[events]
            .iter()
            .map(|event| event.subscribers.len())
            .sum::<usize>()
    };

    assert_eq!(build(2, false), 1, "the live subscription wires");
    assert_eq!(build(2, true), 0, "a disabled subscriber receives nothing");
    assert_eq!(
        build(1, false),
        0,
        "an event the func dropped wires nothing"
    );
}
