use crate::data::static_value::StaticValue;
use crate::data::type_system::DataType;
use crate::graph::Binding;
use crate::graph::BindingEntry;
use crate::graph::Graph;
use crate::graph::address::{InputPort, NodeId};
use crate::graph::definition::GraphDef;
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::node::definition::{FuncId, FuncInput, FuncOutput};
use crate::graph::node::{Node, NodeKind};

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
        .interface
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
            .interface
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
        .interface
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
    assert_eq!(child.interface.inputs[0].name, "B");
    assert_eq!(
        graph.bindings.get(&InputPort::new(instance, 0)),
        Some(&const_int(2)),
        "slot 1 shifted down"
    );

    graph.attach_graph_input(graph_id, detached);
    assert_eq!(graph, original);
}
