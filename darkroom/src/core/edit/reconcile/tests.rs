use super::*;
use scenarium::data::StaticValue;
use scenarium::graph::Node;
use scenarium::prelude::{Graph, SubgraphRef};
use scenarium::testing::{TestFuncHooks, test_func_lib};

fn lib() -> Library {
    test_func_lib(TestFuncHooks::default())
}

fn int_input(name: &str) -> FuncInput {
    FuncInput::optional(name, DataType::Int)
}

/// A subgraph def "S" whose interior is `SubgraphInput`, one `sum`
/// func node (2 inputs, 1 output), and `SubgraphOutput`. Returns the
/// def (interior unwired) plus the three node ids so each test wires
/// it as needed. `authored_inputs`/`outputs` seed the def interface.
fn build_def(
    library: &Library,
    authored_inputs: Vec<FuncInput>,
    authored_outputs: Vec<FuncOutput>,
) -> (SubgraphDef, NodeId, NodeId, NodeId) {
    let sum_id = library.by_name("sum").unwrap().id;
    let sgin = Node::new(NodeKind::SubgraphInput);
    let sum = Node::new(NodeKind::Func(sum_id));
    let sgout = Node::new(NodeKind::SubgraphOutput);
    let (sgin_id, sum_id_n, sgout_id) = (sgin.id, sum.id, sgout.id);
    let mut g = Graph::default();
    g.add(sgin);
    g.add(sum);
    g.add(sgout);
    let def = SubgraphDef::new("00000000-0000-0000-0000-0000000000aa", "S")
        .category("Subgraph")
        .graph(g)
        .inputs(authored_inputs)
        .outputs(authored_outputs);
    (def, sgin_id, sum_id_n, sgout_id)
}

fn bind(graph: &mut Graph, dst_node: NodeId, dst_idx: usize, src_node: NodeId, src_idx: usize) {
    graph.set_input_binding(
        InputPort::new(dst_node, dst_idx),
        Binding::bind(src_node, src_idx),
    );
}

#[test]
fn connecting_placeholder_grows_input_with_inferred_type() {
    // Interior wires sum.in0 <- (SubgraphInput, 0) while def.inputs is
    // empty — the transient state right after a placeholder connect.
    let library = lib();
    let (mut def, sgin, sum, _sgout) = build_def(&library, vec![], vec![]);
    bind(&mut def.graph, sum, 0, sgin, 0);
    let want_ty = library.by_name("sum").unwrap().inputs[0].data_type.clone();
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let def = doc.graph.subgraphs.by_key(&def_id).unwrap();
    assert_eq!(def.inputs.len(), 1, "placeholder use materialized a slot");
    assert_eq!(def.inputs[0].name, "input0");
    assert_eq!(
        def.inputs[0].data_type, want_ty,
        "new slot inherits the consumer's type"
    );
}

#[test]
fn connecting_placeholder_grows_output_with_inferred_type() {
    // (SubgraphOutput, 0) <- sum.out0, def.outputs empty.
    let library = lib();
    let (mut def, _sgin, sum, sgout) = build_def(&library, vec![], vec![]);
    bind(&mut def.graph, sgout, 0, sum, 0);
    let want_ty = library.by_name("sum").unwrap().outputs[0].ty.declared();
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let def = doc.graph.subgraphs.by_key(&def_id).unwrap();
    assert_eq!(def.outputs.len(), 1);
    assert_eq!(def.outputs[0].name, "output0");
    assert_eq!(def.outputs[0].ty.declared(), want_ty);
}

#[test]
fn fully_wired_interface_is_preserved_and_idempotent() {
    // Authored names A,B both used → reconcile must not rename or
    // resize, and a second pass must be a no-op.
    let library = lib();
    let (mut def, sgin, sum, _sgout) =
        build_def(&library, vec![int_input("A"), int_input("B")], vec![]);
    bind(&mut def.graph, sum, 0, sgin, 0);
    bind(&mut def.graph, sum, 1, sgin, 1);
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);
    let names: Vec<String> = doc
        .graph
        .subgraphs
        .by_key(&def_id)
        .unwrap()
        .inputs
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, ["A", "B"], "authored names survive");

    // Idempotent: a second pass changes nothing.
    let before = doc.graph.subgraphs.by_key(&def_id).unwrap().inputs.clone();
    doc.reconcile_boundaries(&library);
    let after = doc.graph.subgraphs.by_key(&def_id).unwrap().inputs.clone();
    assert_eq!(before, after);
}

#[test]
fn middle_disconnect_compacts_interior_and_instance_bindings() {
    // Authored [A,B,C]; interior uses SubgraphInput outputs {0,2}
    // (slot 1 = B is unused). reconcile drops B and renumbers 2 -> 1,
    // rewriting the interior binding AND the instance's bindings.
    let library = lib();
    let (mut def, sgin, sum, _sgout) = build_def(
        &library,
        vec![int_input("A"), int_input("B"), int_input("C")],
        vec![],
    );
    bind(&mut def.graph, sum, 0, sgin, 0); // uses input 0 (A)
    bind(&mut def.graph, sum, 1, sgin, 2); // uses input 2 (C)
    let def_id = def.id;

    let mut graph = Graph::default();
    graph.subgraphs.add(def.clone());
    let inst = graph.add_subgraph_node(&def, SubgraphRef::Local(def_id));
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
        .subgraphs
        .by_key(&def_id)
        .unwrap()
        .inputs
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, ["A", "C"]);

    // Interior: sum.in0 still from slot 0, sum.in1 now from slot 1
    // (was 2).
    let interior = &doc.graph.subgraphs.by_key(&def_id).unwrap().graph;
    assert_eq!(
        interior.input_binding(InputPort::new(sum, 0)),
        Binding::bind(sgin, 0),
    );
    assert_eq!(
        interior.input_binding(InputPort::new(sum, 1)),
        Binding::bind(sgin, 1),
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
fn unused_subgraph_input_shrinks_interface() {
    // Authored [A,B] but only output 0 is wired → B is dropped.
    let library = lib();
    let (mut def, sgin, sum, _sgout) =
        build_def(&library, vec![int_input("A"), int_input("B")], vec![]);
    bind(&mut def.graph, sum, 0, sgin, 0);
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let inputs = &doc.graph.subgraphs.by_key(&def_id).unwrap().inputs;
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
    let (mut def, sgin, sum, _sgout) = build_def(&library, vec![stale], vec![]);
    bind(&mut def.graph, sum, 0, sgin, 0); // sgin.out0 -> sum.in0
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let input = &doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0];
    assert_eq!(input.name, "A", "authored name preserved");
    assert_eq!(
        input.data_type,
        DataType::Int,
        "type re-derived from the wired func input, replacing the stale Bool"
    );
}

#[test]
fn passthrough_ports_are_null_typed() {
    // `SubgraphInput.out0` wired straight to `SubgraphOutput.in0` — no
    // real func between. Both boundary ports are polymorphic, so their
    // derived type is `Null`.
    let library = lib();
    let sgin = Node::new(NodeKind::SubgraphInput);
    let sgout = Node::new(NodeKind::SubgraphOutput);
    let (sgin_id, sgout_id) = (sgin.id, sgout.id);
    let mut interior = Graph::default();
    interior.add(sgin);
    interior.add(sgout);
    // sgout.in0 <- sgin.out0
    interior.set_input_binding(InputPort::new(sgout_id, 0), Binding::bind(sgin_id, 0));
    let def = SubgraphDef::new("00000000-0000-0000-0000-0000000000dd", "Pass")
        .category("Subgraph")
        .graph(interior);
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let def = doc.graph.subgraphs.by_key(&def_id).unwrap();
    assert_eq!(def.inputs.len(), 1);
    assert_eq!(def.outputs.len(), 1);
    assert_eq!(
        def.inputs[0].data_type,
        DataType::Null,
        "a passthrough subgraph input is polymorphic (Null)"
    );
    assert_eq!(def.outputs[0].ty.declared(), DataType::Null);
}

#[test]
fn passthrough_in_subgraph_exposes_the_resolved_output_type() {
    use scenarium::node::special::SpecialNode;

    // Interior: SubgraphInput → sum → CachePassthrough → SubgraphOutput. The
    // passthrough's output statically declares the wildcard `Null`, but the
    // exposed subgraph output must report `sum`'s real output type, resolved
    // through it — otherwise wrapping a value in a file-cache would erase its
    // type for the whole composite.
    let library = lib();
    let sum_id = library.by_name("sum").unwrap().id;
    let want_ty = library.by_name("sum").unwrap().outputs[0].ty.declared();
    assert_ne!(
        want_ty,
        DataType::Null,
        "fixture sanity: sum has a concrete output type"
    );

    let sgin = Node::new(NodeKind::SubgraphInput);
    let sum = Node::new(NodeKind::Func(sum_id));
    let pass = Node::new(NodeKind::Special(SpecialNode::CachePassthrough {
        bypass: false,
    }));
    let sgout = Node::new(NodeKind::SubgraphOutput);
    let (sgin_id, sum_n, pass_id, sgout_id) = (sgin.id, sum.id, pass.id, sgout.id);
    let mut interior = Graph::default();
    interior.add(sgin);
    interior.add(sum);
    interior.add(pass);
    interior.add(sgout);
    // sum reads the two boundary inputs; the passthrough caches sum's output;
    // the boundary output exposes the passthrough.
    bind(&mut interior, sum_n, 0, sgin_id, 0);
    bind(&mut interior, sum_n, 1, sgin_id, 1);
    bind(&mut interior, pass_id, 0, sum_n, 0);
    bind(&mut interior, sgout_id, 0, pass_id, 0);

    let def = SubgraphDef::new("00000000-0000-0000-0000-0000000000ee", "PassSum")
        .category("Subgraph")
        .graph(interior);
    let def_id = def.id;
    let mut graph = Graph::default();
    graph.subgraphs.add(def);
    let mut doc: Document = graph.into();

    doc.reconcile_boundaries(&library);

    let def = doc.graph.subgraphs.by_key(&def_id).unwrap();
    assert_eq!(def.outputs.len(), 1);
    assert_eq!(
        def.outputs[0].ty.declared(),
        want_ty,
        "the exposed output must keep the type resolved through the passthrough"
    );
}
