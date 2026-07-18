use super::*;
use scenarium::Graph;
use scenarium::GraphLink;
use scenarium::Node;
use scenarium::StaticValue;
use scenarium::testing::{TestFuncHooks, test_func_lib};
use scenarium::{Func, FuncId};

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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let graph = doc.graph.graphs.get(&graph_id).unwrap();
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let graph = doc.graph.graphs.get(&graph_id).unwrap();
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);
    let names: Vec<String> = doc
        .graph
        .graphs
        .get(&graph_id)
        .unwrap()
        .inputs
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, ["A", "B"], "authored names survive");

    // Idempotent: a second pass changes nothing.
    let before = doc.graph.graphs.get(&graph_id).unwrap().inputs.clone();
    doc.reconcile_boundaries(&library);
    let after = doc.graph.graphs.get(&graph_id).unwrap().inputs.clone();
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    // Interface compacted to [A, C].
    let names: Vec<String> = doc
        .graph
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
    let interior = doc.graph.graphs.get(&graph_id).unwrap();
    assert_eq!(
        interior.input_binding(InputPort::new(fixture.sum, 0)),
        Binding::bind(fixture.input, 0),
    );
    assert_eq!(
        interior.input_binding(InputPort::new(fixture.sum, 1)),
        Binding::bind(fixture.input, 1),
    );

    // Instance: old slot 0 stays (10), old slot 2 -> new slot 1 (12),
    // dropped slot 1 (11) is cleared.
    assert_eq!(
        doc.graph.input_binding(InputPort::new(inst, 0)),
        Binding::Const(StaticValue::Int(10)),
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(inst, 1)),
        Binding::Const(StaticValue::Int(12)),
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(inst, 2)),
        Binding::None,
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let inputs = &doc.graph.graphs.get(&graph_id).unwrap().inputs;
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let input = &doc.graph.graphs.get(&graph_id).unwrap().inputs[0];
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let graph = doc.graph.graphs.get(&graph_id).unwrap();
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
    library.funcs.add(pass_func.clone());
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
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let graph = doc.graph.graphs.get(&graph_id).unwrap();
    assert_eq!(graph.outputs.len(), 1);
    assert_eq!(
        graph.outputs[0].ty.declared(),
        want_ty,
        "the exposed output must keep the type resolved through the passthrough"
    );
}
