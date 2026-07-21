use crate::data::static_value::StaticValue;
use crate::data::type_system::DataType;
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeKind};
use crate::library::Library;
use crate::node::definition::{Func, FuncId, FuncInput, FuncOutput};
use crate::node::event::EventLambda;
use crate::testing::{TestFuncHooks, test_func_lib};

fn lib() -> Library {
    test_func_lib(TestFuncHooks::default())
}

fn int_input(name: &str) -> FuncInput {
    FuncInput::optional(name, DataType::Int)
}

#[derive(Debug)]
struct GraphFixture {
    graph: Graph,
    input: NodeId,
    sum: NodeId,
    output: NodeId,
}

fn build_graph(
    library: &Library,
    authored_inputs: Vec<FuncInput>,
    authored_outputs: Vec<FuncOutput>,
) -> GraphFixture {
    let sum_id = library.by_name("sum").unwrap().id;
    let sgin = Node::new(NodeKind::GraphInput);
    let sum = Node::new(NodeKind::Func(sum_id));
    let sgout = Node::new(NodeKind::GraphOutput);
    let mut graph = Graph::new("S")
        .category("Graph")
        .inputs(authored_inputs)
        .outputs(authored_outputs);
    let input = graph.add(sgin);
    let sum = graph.add(sum);
    let output = graph.add(sgout);
    GraphFixture {
        graph,
        input,
        sum,
        output,
    }
}

fn bind(graph: &mut Graph, dst_node: NodeId, dst_idx: usize, src_node: NodeId, src_idx: usize) {
    graph.set_input_binding(
        InputPort::new(dst_node, dst_idx),
        Binding::bind(src_node, src_idx),
    );
}

#[test]
fn connecting_placeholder_grows_input_with_inferred_type() {
    // Interior wires sum.in0 <- (GraphInput, 0) while def.inputs is
    // empty — the transient state right after a placeholder connect.
    let library = lib();
    let mut fixture = build_graph(&library, vec![], vec![]);
    bind(&mut fixture.graph, fixture.sum, 0, fixture.input, 0);
    let want_ty = library.by_name("sum").unwrap().inputs[0].data_type.clone();
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, fixture.graph);

    graph.normalize(&library);

    let graph = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(graph.inputs.len(), 1, "placeholder use materialized a slot");
    assert_eq!(graph.inputs[0].name, "input0");
    assert_eq!(
        graph.inputs[0].data_type, want_ty,
        "new slot inherits the consumer's type"
    );
}

#[test]
fn connecting_placeholder_grows_output_with_inferred_type() {
    // (GraphOutput, 0) <- sum.out0, def.outputs empty.
    let library = lib();
    let mut fixture = build_graph(&library, vec![], vec![]);
    bind(&mut fixture.graph, fixture.output, 0, fixture.sum, 0);
    let want_ty = library.by_name("sum").unwrap().outputs[0].ty.declared();
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, fixture.graph);

    graph.normalize(&library);

    let graph = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(graph.outputs.len(), 1);
    assert_eq!(graph.outputs[0].name, "output0");
    assert_eq!(graph.outputs[0].ty.declared(), want_ty);
}

#[test]
fn fully_wired_interface_is_preserved_and_idempotent() {
    // Authored names A,B both used → reconcile must not rename or
    // resize, and a second pass must be a no-op.
    let library = lib();
    let mut fixture = build_graph(&library, vec![int_input("A"), int_input("B")], vec![]);
    bind(&mut fixture.graph, fixture.sum, 0, fixture.input, 0);
    bind(&mut fixture.graph, fixture.sum, 1, fixture.input, 1);
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, fixture.graph);

    graph.normalize(&library);
    let names: Vec<String> = graph
        .graphs
        .get(&graph_id)
        .unwrap()
        .inputs
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, ["A", "B"], "authored names survive");

    // Idempotent: a second pass changes nothing.
    let before = graph.graphs.get(&graph_id).unwrap().inputs.clone();
    graph.normalize(&library);
    let after = graph.graphs.get(&graph_id).unwrap().inputs.clone();
    assert_eq!(before, after);
}

#[test]
fn middle_disconnect_compacts_interior_and_instance_bindings() {
    // Authored [A,B,C]; interior uses GraphInput outputs {0,2}
    // (slot 1 = B is unused). reconcile drops B and renumbers 2 -> 1,
    // rewriting the interior binding AND the instance's bindings.
    let library = lib();
    let mut fixture = build_graph(
        &library,
        vec![int_input("A"), int_input("B"), int_input("C")],
        vec![],
    );
    bind(&mut fixture.graph, fixture.sum, 0, fixture.input, 0); // uses input 0 (A)
    bind(&mut fixture.graph, fixture.sum, 1, fixture.input, 2); // uses input 2 (C)
    let graph_id = GraphId::unique();

    let mut graph = Graph::default();
    graph.insert_graph(graph_id, fixture.graph.clone());
    let inst = graph.add_graph_node(&fixture.graph, GraphLink::Local(graph_id));
    // Instance bindings on all three inputs, distinguishable by value.
    graph.set_input_binding(
        InputPort::new(inst, 0),
        Binding::Const(StaticValue::Int(10)),
    );
    graph.set_input_binding(
        InputPort::new(inst, 1),
        Binding::Const(StaticValue::Int(11)),
    );
    graph.set_input_binding(
        InputPort::new(inst, 2),
        Binding::Const(StaticValue::Int(12)),
    );
    graph.normalize(&library);

    // Interface compacted to [A, C].
    let names: Vec<String> = graph
        .graphs
        .get(&graph_id)
        .unwrap()
        .inputs
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, ["A", "C"]);

    // Interior: sum.in0 still from slot 0, sum.in1 now from slot 1
    // (was 2).
    let interior = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(
        interior.bindings.get(&InputPort::new(fixture.sum, 0)),
        Some(&Binding::bind(fixture.input, 0)),
    );
    assert_eq!(
        interior.bindings.get(&InputPort::new(fixture.sum, 1)),
        Some(&Binding::bind(fixture.input, 1)),
    );

    // Instance: old slot 0 stays (10), old slot 2 -> new slot 1 (12),
    // dropped slot 1 (11) is cleared.
    assert_eq!(
        graph.bindings.get(&InputPort::new(inst, 0)),
        Some(&Binding::Const(StaticValue::Int(10))),
    );
    assert_eq!(
        graph.bindings.get(&InputPort::new(inst, 1)),
        Some(&Binding::Const(StaticValue::Int(12))),
    );
    assert_eq!(graph.bindings.get(&InputPort::new(inst, 2)), None,);
}

#[test]
fn nested_local_graphs_normalize_within_their_owning_scope() {
    let library = lib();
    let repeated_id = GraphId::unique();

    let mut direct = build_graph(
        &library,
        vec![int_input("Direct A"), int_input("Direct B")],
        vec![],
    );
    bind(&mut direct.graph, direct.sum, 0, direct.input, 1);

    let mut nested = build_graph(
        &library,
        vec![
            int_input("Nested A"),
            int_input("Nested B"),
            int_input("Nested C"),
        ],
        vec![],
    );
    bind(&mut nested.graph, nested.sum, 0, nested.input, 0);
    bind(&mut nested.graph, nested.sum, 1, nested.input, 2);

    let mut outer = Graph::new("Outer");
    outer.insert_graph(repeated_id, nested.graph.clone());
    let nested_instance = outer.add_graph_node(&nested.graph, GraphLink::Local(repeated_id));
    for (port_idx, value) in [20, 21, 22].into_iter().enumerate() {
        outer.set_input_binding(
            InputPort::new(nested_instance, port_idx),
            Binding::Const(StaticValue::Int(value)),
        );
    }

    let outer_id = GraphId::unique();
    let mut root = Graph::default();
    root.insert_graph(repeated_id, direct.graph.clone());
    let direct_instance = root.add_graph_node(&direct.graph, GraphLink::Local(repeated_id));
    root.set_input_binding(
        InputPort::new(direct_instance, 0),
        Binding::Const(StaticValue::Int(10)),
    );
    root.set_input_binding(
        InputPort::new(direct_instance, 1),
        Binding::Const(StaticValue::Int(11)),
    );
    root.insert_graph(outer_id, outer);

    root.normalize(&library);

    let direct = root.graphs.get(&repeated_id).unwrap();
    assert_eq!(direct.inputs[0].name, "Direct B");
    assert_eq!(direct.inputs.len(), 1);
    assert_eq!(
        root.bindings.get(&InputPort::new(direct_instance, 0)),
        Some(&Binding::Const(StaticValue::Int(11))),
    );
    assert_eq!(root.bindings.get(&InputPort::new(direct_instance, 1)), None,);

    let outer = root.graphs.get(&outer_id).unwrap();
    let nested = outer.graphs.get(&repeated_id).unwrap();
    let nested_names: Vec<&str> = nested
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    assert_eq!(nested_names, ["Nested A", "Nested C"]);
    assert_eq!(
        outer.bindings.get(&InputPort::new(nested_instance, 0)),
        Some(&Binding::Const(StaticValue::Int(20))),
    );
    assert_eq!(
        outer.bindings.get(&InputPort::new(nested_instance, 1)),
        Some(&Binding::Const(StaticValue::Int(22))),
    );
    assert_eq!(
        outer.bindings.get(&InputPort::new(nested_instance, 2)),
        None,
    );
}

#[test]
fn unused_graph_input_shrinks_interface() {
    // Authored [A,B] but only output 0 is wired → B is dropped.
    let library = lib();
    let mut fixture = build_graph(&library, vec![int_input("A"), int_input("B")], vec![]);
    bind(&mut fixture.graph, fixture.sum, 0, fixture.input, 0);
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, fixture.graph);

    graph.normalize(&library);

    let inputs = &graph.graphs.get(&graph_id).unwrap().inputs;
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].name, "A");
}

#[test]
fn existing_port_type_is_rederived_from_wiring() {
    // An existing slot authored with a stale type (Bool) but wired to
    // `sum.in0` (Int): reconcile corrects the type to Int while keeping
    // the authored name.
    let library = lib();
    let stale = FuncInput::optional("A", DataType::Bool);
    let mut fixture = build_graph(&library, vec![stale], vec![]);
    bind(&mut fixture.graph, fixture.sum, 0, fixture.input, 0);
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, fixture.graph);

    graph.normalize(&library);

    let input = &graph.graphs.get(&graph_id).unwrap().inputs[0];
    assert_eq!(input.name, "A", "authored name preserved");
    assert_eq!(
        input.data_type,
        DataType::Int,
        "type re-derived from the wired func input, replacing the stale Bool"
    );
}

#[test]
fn passthrough_ports_are_null_typed() {
    // `GraphInput.out0` wired straight to `GraphOutput.in0` — no
    // real func between. Both boundary ports are polymorphic, so their
    // derived type is `Any`.
    let library = lib();
    let sgin = Node::new(NodeKind::GraphInput);
    let sgout = Node::new(NodeKind::GraphOutput);
    let mut interior = Graph::default();
    let sgin_id = interior.add(sgin);
    let sgout_id = interior.add(sgout);
    // sgout.in0 <- sgin.out0
    interior.set_input_binding(InputPort::new(sgout_id, 0), Binding::bind(sgin_id, 0));
    interior.name = "Pass".into();
    interior.category = "Graph".into();
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, interior);

    graph.normalize(&library);

    let graph = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(graph.inputs.len(), 1);
    assert_eq!(graph.outputs.len(), 1);
    assert_eq!(
        graph.inputs[0].data_type,
        DataType::Any,
        "a passthrough graph input is polymorphic (Null)"
    );
    assert_eq!(graph.outputs[0].ty.declared(), DataType::Any);
}

#[test]
fn passthrough_in_graph_exposes_the_resolved_output_type() {
    // Interior: GraphInput → sum → wildcard passthrough → GraphOutput. The
    // passthrough's output statically declares the wildcard `Any`, but the
    // exposed graph output must report `sum`'s real output type, resolved
    // through it — otherwise wrapping a value in a passthrough would erase its
    // type for the whole composite.
    let pass_func = Func::new(FuncId::unique(), "pass")
        .input(FuncInput::required("x", DataType::Any))
        .wildcard_output("o", 0);
    let mut library = lib();
    library.add(pass_func.clone());
    let sum_id = library.by_name("sum").unwrap().id;
    let want_ty = library.by_name("sum").unwrap().outputs[0].ty.declared();
    assert_ne!(
        want_ty,
        DataType::Any,
        "fixture sanity: sum has a concrete output type"
    );

    let sgin = Node::new(NodeKind::GraphInput);
    let sum = Node::new(NodeKind::Func(sum_id));
    let pass = Node::from(&pass_func);
    let sgout = Node::new(NodeKind::GraphOutput);
    let mut interior = Graph::default();
    let sgin_id = interior.add(sgin);
    let sum_n = interior.add(sum);
    let pass_id = interior.add(pass);
    let sgout_id = interior.add(sgout);
    // sum reads the two boundary inputs; the passthrough caches sum's output;
    // the boundary output exposes the passthrough.
    bind(&mut interior, sum_n, 0, sgin_id, 0);
    bind(&mut interior, sum_n, 1, sgin_id, 1);
    bind(&mut interior, pass_id, 0, sum_n, 0);
    bind(&mut interior, sgout_id, 0, pass_id, 0);

    interior.name = "PassSum".into();
    interior.category = "Graph".into();
    let graph_id = GraphId::unique();
    let mut graph = Graph::default();
    graph.insert_graph(graph_id, interior);

    graph.normalize(&library);

    let graph = graph.graphs.get(&graph_id).unwrap();
    assert_eq!(graph.outputs.len(), 1);
    assert_eq!(
        graph.outputs[0].ty.declared(),
        want_ty,
        "the exposed output must keep the type resolved through the passthrough"
    );
}

#[test]
fn normalize_drops_out_of_range_and_missing_emitter_subscriptions() {
    let func_id = FuncId::from_u128(0xe0e0);
    let mut library = Library::default();
    library.add(Func::new(func_id, "emitter").event("tick", EventLambda::default()));

    let mut graph = Graph::default();
    let emitter_id = graph.add(Node::new(NodeKind::Func(func_id)));
    let subscriber_id = graph.add(Node::new(NodeKind::Func(func_id)));
    let missing_emitter = NodeId::unique();
    graph.subscribe(emitter_id, 0, subscriber_id);
    graph.subscribe(emitter_id, 1, subscriber_id);
    graph.subscribe(missing_emitter, 0, subscriber_id);

    graph.normalize(&library);

    assert!(graph.is_subscribed(emitter_id, 0, subscriber_id));
    assert!(!graph.is_subscribed(emitter_id, 1, subscriber_id));
    assert!(!graph.is_subscribed(missing_emitter, 0, subscriber_id));

    graph.normalize(&library);
    assert_eq!(graph.subscriptions().count(), 1);

    let unresolved_id = graph.add(Node::new(NodeKind::Func(FuncId::from_u128(0xdead))));
    graph.subscribe(unresolved_id, 4, subscriber_id);
    graph.normalize(&library);
    assert!(graph.is_subscribed(unresolved_id, 4, subscriber_id));
}

#[test]
fn normalize_drops_out_of_range_and_missing_binding_endpoints() {
    let func_id = FuncId::from_u128(0xb12d);
    let mut library = Library::default();
    library.add(
        Func::new(func_id, "op")
            .input(FuncInput::optional("in", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int)),
    );

    let mut graph = Graph::default();
    let ids: Vec<NodeId> = (0..5)
        .map(|_| graph.add(Node::new(NodeKind::Func(func_id))))
        .collect();
    let (a, b, c, d, e) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let missing_node = NodeId::unique();
    graph.inputs.push(FuncInput::optional("in", DataType::Int));
    graph.outputs.push(FuncOutput::new("out", DataType::Int));
    let graph_input = graph.add(Node::new(NodeKind::GraphInput));
    let graph_output = graph.add(Node::new(NodeKind::GraphOutput));

    graph.set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    graph.set_input_binding(InputPort::new(c, 5), Binding::bind(a, 0));
    graph.set_input_binding(InputPort::new(d, 0), Binding::bind(a, 9));
    graph.set_input_binding(InputPort::new(e, 0), Binding::bind(missing_node, 0));
    graph.set_input_binding(InputPort::new(missing_node, 0), Binding::bind(a, 0));
    graph.set_input_binding(
        InputPort::new(graph_output, 0),
        Binding::bind(graph_input, 0),
    );
    graph.set_input_binding(
        InputPort::new(graph_output, 1),
        Binding::bind(graph_input, 0),
    );

    graph.normalize(&library);

    assert_eq!(
        graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::bind(a, 0)),
    );
    for dead in [
        InputPort::new(c, 5),
        InputPort::new(d, 0),
        InputPort::new(e, 0),
        InputPort::new(missing_node, 0),
        InputPort::new(graph_output, 1),
    ] {
        assert!(!graph.bindings.contains_key(&dead));
    }
    assert_eq!(
        graph.bindings.get(&InputPort::new(graph_output, 0)),
        Some(&Binding::bind(graph_input, 0)),
    );

    graph.set_input_binding(
        InputPort::new(b, 0),
        Binding::Const(StaticValue::from(1i64)),
    );
    graph.normalize(&library);
    assert_eq!(
        graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::Const(StaticValue::from(1i64))),
    );

    let unresolved_id = graph.add(Node::new(NodeKind::Func(FuncId::from_u128(0xdead))));
    graph.set_input_binding(InputPort::new(unresolved_id, 3), Binding::bind(a, 0));
    graph.set_input_binding(InputPort::new(b, 0), Binding::bind(unresolved_id, 7));
    graph.normalize(&library);
    assert_eq!(
        graph.bindings.get(&InputPort::new(unresolved_id, 3)),
        Some(&Binding::bind(a, 0)),
    );
    assert_eq!(
        graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::bind(unresolved_id, 7)),
    );
}
