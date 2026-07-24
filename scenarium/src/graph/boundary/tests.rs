use crate::data::static_value::StaticValue;
use crate::data::type_system::DataType;
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::wiring::BindingEntry;
use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeKind, OutputPort};
use crate::node::definition::{FuncId, FuncInput, FuncOutput};

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
    let mut child = Graph::new("child").inputs([int_input("A"), int_input("B"), int_input("C")]);
    let boundary = child.add(Node::new(NodeKind::GraphInput));
    let consumer = child.add(func_node());
    for idx in 0..3 {
        child.set_input_binding(InputPort::new(consumer, idx), Binding::bind(boundary, idx));
    }
    child.set_output_pinned(OutputPort::new(boundary, 1), true);
    child.set_output_pinned(OutputPort::new(boundary, 2), true);

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
    let original = graph.clone();

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
    assert_eq!(detached.pins, vec![OutputPort::new(boundary, 1)]);
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
        .definition
        .as_ref()
        .unwrap()
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    assert_eq!(names, ["A", "C"]);
    // Interior: in0 keeps slot 0, in1's edge was severed, in2's source
    // shifted 2 -> 1.
    assert_eq!(
        child.bindings.get(&InputPort::new(consumer, 0)),
        Some(&Binding::bind(boundary, 0))
    );
    assert_eq!(child.bindings.get(&InputPort::new(consumer, 1)), None);
    assert_eq!(
        child.bindings.get(&InputPort::new(consumer, 2)),
        Some(&Binding::bind(boundary, 1))
    );
    // Pins: 1 dropped, 2 shifted to 1.
    let pins: Vec<OutputPort> = child.pinned_outputs().collect();
    assert_eq!(pins, vec![OutputPort::new(boundary, 1)]);
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
            .definition
            .as_ref()
            .unwrap()
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
        Graph::new("child").outputs([int_output("X"), int_output("Y"), int_output("Z")]);
    let boundary = child.add(Node::new(NodeKind::GraphOutput));
    let producer = child.add(func_node());
    child.set_input_binding(InputPort::new(boundary, 0), Binding::bind(producer, 0));
    child.set_input_binding(InputPort::new(boundary, 1), Binding::bind(producer, 0));
    child.set_input_binding(InputPort::new(boundary, 2), Binding::bind(producer, 1));

    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance = graph.add(Node::graph_instance(&child, GraphLink::Local(graph_id)));
    let consumer_a = graph.add(func_node());
    let consumer_b = graph.add(func_node());
    graph.insert_graph(graph_id, child);
    graph.set_input_binding(InputPort::new(consumer_a, 0), Binding::bind(instance, 1));
    graph.set_input_binding(InputPort::new(consumer_b, 0), Binding::bind(instance, 2));
    graph.set_output_pinned(OutputPort::new(instance, 1), true);
    graph.set_output_pinned(OutputPort::new(instance, 2), true);
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
    let original = graph.clone();

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
    assert_eq!(detached.pins, vec![OutputPort::new(instance, 1)]);
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
        .definition
        .as_ref()
        .unwrap()
        .outputs
        .iter()
        .map(|output| output.name.as_str())
        .collect();
    assert_eq!(names, ["X", "Z"]);
    // Interior: slot 1's binding removed, slot 2's rekeyed to 1.
    assert_eq!(
        child.bindings.get(&InputPort::new(boundary, 0)),
        Some(&Binding::bind(producer, 0))
    );
    assert_eq!(
        child.bindings.get(&InputPort::new(boundary, 1)),
        Some(&Binding::bind(producer, 1))
    );
    assert_eq!(child.bindings.get(&InputPort::new(boundary, 2)), None);
    // Parent: consumer A severed, consumer B's source shifted 2 -> 1,
    // pin 1 dropped and pin 2 shifted to 1.
    assert_eq!(graph.bindings.get(&InputPort::new(consumer_a, 0)), None);
    assert_eq!(
        graph.bindings.get(&InputPort::new(consumer_b, 0)),
        Some(&Binding::bind(instance, 1))
    );
    let pins: Vec<OutputPort> = graph.pinned_outputs().collect();
    assert_eq!(pins, vec![OutputPort::new(instance, 1)]);

    graph.attach_graph_output(graph_id, detached);
    assert_eq!(
        graph, original,
        "attach restores the exact pre-detach graph"
    );
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
    let child = Graph::new("bare").inputs([int_input("A"), int_input("B")]);
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance = graph.add(Node::graph_instance(&child, GraphLink::Local(graph_id)));
    graph.insert_graph(graph_id, child);
    graph.set_input_binding(InputPort::new(instance, 0), const_int(1));
    graph.set_input_binding(InputPort::new(instance, 1), const_int(2));
    let original = graph.clone();

    let detached = graph.detach_graph_input(graph_id, 0);
    assert!(detached.interior.is_empty() && detached.pins.is_empty());
    assert_eq!(detached.parent.len(), 1);
    let child = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(child.definition.as_ref().unwrap().inputs[0].name, "B");
    assert_eq!(
        graph.bindings.get(&InputPort::new(instance, 0)),
        Some(&const_int(2)),
        "slot 1 shifted down"
    );

    graph.attach_graph_input(graph_id, detached);
    assert_eq!(graph, original);
}
