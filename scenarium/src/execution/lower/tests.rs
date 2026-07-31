use crate::execution::lower::Lowerer;
use crate::execution::lower::lowered_graph::{LoweredBinding, LoweredGraph};
use crate::graph::func::{Func, FuncInput, FuncOutput};
use crate::graph::identity::{FuncId, InputPort, NodeId};
use crate::graph::{Binding, Graph};
use crate::library::Library;
use crate::testing;
use crate::{DataType, StaticValue};

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

/// A producer and a consumer, optionally wired, lowered. Returns the result
/// alongside both node ids so a test can name either end.
fn wired(library: &Library, binding: Option<Binding>) -> (LoweredGraph, NodeId, NodeId) {
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let consumer = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    if let Some(binding) = binding {
        graph.set_input_binding(InputPort::new(consumer, 0), binding);
    }
    let mut lowered = LoweredGraph::default();
    Lowerer::default().lower(&graph, library, &mut lowered);
    (lowered, producer, consumer)
}

/// The position a node landed at in emit order. Emit order is `Graph::iter`'s,
/// which is a `HashMap` walk — so a test names a node by id, never by index.
fn at(lowered: &LoweredGraph, node_id: NodeId) -> usize {
    lowered
        .node_ids
        .iter()
        .position(|id| *id == node_id)
        .expect("the walk emits every authored node")
}

/// Every authored node becomes exactly one execution node carrying its own id —
/// the whole of the authoring↔execution projection now that nothing dissolves.
#[test]
fn emits_one_execution_node_per_authored_node() {
    let library = library(DataType::Int, DataType::Int);
    let (lowered, producer, consumer) = wired(&library, None);

    assert_eq!(lowered.e_nodes.len(), 2);
    let mut ids: Vec<NodeId> = lowered.node_ids.clone();
    ids.sort();
    let mut expected = vec![producer, consumer];
    expected.sort();
    assert_eq!(ids, expected);
}

/// A node's port runs come from its func's arity and are packed in emit order —
/// what lets linking rebuild the columns slot for slot.
#[test]
fn packs_one_port_run_per_node_from_its_declaration() {
    let library = library(DataType::Int, DataType::Int);
    let (lowered, producer, consumer) = wired(&library, None);

    let producer_node = &lowered.e_nodes[at(&lowered, producer)];
    let consumer_node = &lowered.e_nodes[at(&lowered, consumer)];
    assert_eq!(
        (producer_node.outputs.len, producer_node.inputs.len),
        (1, 0)
    );
    assert_eq!(
        (consumer_node.outputs.len, consumer_node.inputs.len),
        (0, 1)
    );
    assert_eq!(
        lowered.outputs.len(),
        1,
        "one output port across both nodes, typed from the producer's declaration"
    );
    assert_eq!(lowered.outputs[producer_node.outputs][0], DataType::Int);
    assert_eq!(lowered.inputs.len(), 1);
    assert_eq!(
        producer_node.outputs.start, 0,
        "the only run starts the pool"
    );
}

/// A wire resolves to the producer it names, by id — the one thing about a port
/// the walk settles and linking translates.
#[test]
fn resolves_a_wire_to_the_producer_it_names() {
    let library = library(DataType::Int, DataType::Int);
    let mut graph = Graph::default();
    let producer = graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let consumer = graph.add_func_node(library.by_id(FuncId::from_u128(CONSUMER)).unwrap());
    graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    let mut lowered = LoweredGraph::default();
    Lowerer::default().lower(&graph, &library, &mut lowered);

    let consumer_node = &lowered.e_nodes[at(&lowered, consumer)];
    let input = &lowered.inputs[consumer_node.inputs][0];
    match &input.binding {
        LoweredBinding::Bind(port) => {
            assert_eq!(port.node_id, producer);
            assert_eq!(port.port_idx, 0);
        }
        other => panic!("expected a bind, got {other:?}"),
    }
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
    let mut lowered = LoweredGraph::default();
    Lowerer::default().lower(&graph, &library, &mut lowered);

    let consumer_node = &lowered.e_nodes[at(&lowered, consumer)];
    assert!(
        matches!(
            lowered.inputs[consumer_node.inputs][0].binding,
            LoweredBinding::None
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
        let (lowered, _, consumer) = wired(&library, Some(Binding::Const(value)));
        let consumer_node = &lowered.e_nodes[at(&lowered, consumer)];
        assert_eq!(
            matches!(
                lowered.inputs[consumer_node.inputs][0].binding,
                LoweredBinding::Const(_)
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
    let mut lowered = LoweredGraph::default();
    Lowerer::default().lower(&graph, &library, &mut lowered);

    assert_eq!(lowered.e_nodes.len(), 1);
    assert!(lowered.e_nodes[at(&lowered, producer)].disabled);
}

/// The walk clears its buffer on entry, so one `LoweredGraph` serves every
/// compile and a second lowering cannot observe the first.
#[test]
fn a_second_lowering_cannot_observe_the_first() {
    let library = library(DataType::Int, DataType::Int);
    let mut graph = Graph::default();
    graph.add_func_node(library.by_id(FuncId::from_u128(PRODUCER)).unwrap());
    let mut lowered = LoweredGraph::default();
    let mut lowerer = Lowerer::default();
    lowerer.lower(&graph, &library, &mut lowered);
    lowerer.lower(&graph, &library, &mut lowered);

    assert_eq!(lowered.e_nodes.len(), 1);
    assert_eq!(
        lowered.outputs.len(),
        1,
        "the port pools restart from empty"
    );
    assert!(lowered.subscriptions.is_empty());
}
