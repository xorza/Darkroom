//! The startup demo graph darkroom opens with when there's no persisted
//! document — the shared `test_graph` fixture plus one local subgraph
//! instance, so the default scene exercises both func and composite nodes.

use scenarium::data::DataType;
use scenarium::function::{FuncInput, FuncOutput};
use scenarium::prelude::{
    Graph as CoreGraph, InputPort, Node, NodeId, NodeKind, SubgraphDef, SubgraphRef,
};
use scenarium::testing::{TestFuncHooks, test_func_lib, test_graph};

/// The shared `test_graph` fixture plus one local subgraph instance
/// ("Adder", an `in(A,B) -> sum -> out(Sum)` composite) wired off the
/// existing `get_a` / `get_b` source nodes — so the default scene shows
/// a composite node alongside the func nodes.
pub fn sample_graph() -> CoreGraph {
    let mut graph = test_graph();

    let func_lib = test_func_lib(TestFuncHooks::default());
    let sum_id = func_lib.by_name("sum").unwrap().id;

    // Interior: SubgraphInput(A, B) -> sum -> SubgraphOutput(Sum).
    let in_node = Node::new(NodeKind::SubgraphInput);
    let in_id = in_node.id;
    let sum_node = Node::new(NodeKind::Func(sum_id));
    let sum_node_id = sum_node.id;
    let out_node = Node::new(NodeKind::SubgraphOutput);
    let out_id = out_node.id;

    let mut inner = CoreGraph::default();
    inner.add(in_node);
    inner.add(sum_node);
    inner.add(out_node);
    inner.set_input_binding(InputPort::new(sum_node_id, 0), (in_id, 0).into());
    inner.set_input_binding(InputPort::new(sum_node_id, 1), (in_id, 1).into());
    inner.set_input_binding(InputPort::new(out_id, 0), (sum_node_id, 0).into());

    let def = SubgraphDef {
        id: "c1a9e3d2-5b47-4f8a-9d3e-2f6b8c1a4e90".into(),
        name: "Adder".into(),
        category: "Subgraph".into(),
        graph: inner,
        inputs: vec![subgraph_input("A"), subgraph_input("B")],
        outputs: vec![FuncOutput {
            name: "Sum".into(),
            data_type: DataType::Int,
        }],
        events: vec![],
    };
    let def_ref = SubgraphRef::Local(def.id);

    let mut inst = Node::subgraph_instance(&def, def_ref);
    inst.id = "a7f2b914-6c3d-4e85-bb12-9d4f7a3c0e21".into();
    let inst_id = inst.id;
    graph.subgraphs.add(def);
    graph.add(inst);

    let get_a_node_id: NodeId = "5f110618-8faa-4629-8f5d-473c236de7d1".into();
    let get_b_node_id: NodeId = "6fc6b533-c375-451c-ba3a-a14ea217cb30".into();
    graph.set_input_binding(InputPort::new(inst_id, 0), (get_a_node_id, 0).into());
    graph.set_input_binding(InputPort::new(inst_id, 1), (get_b_node_id, 0).into());

    graph.validate();
    graph
}

fn subgraph_input(name: &str) -> FuncInput {
    FuncInput {
        name: name.into(),
        required: false,
        data_type: DataType::Int,
        default_value: None,
        value_options: vec![],
    }
}
