use crate::data::static_value::StaticValue;
use crate::data::type_system::DataType;
use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeKind};
use crate::library::Library;
use crate::node::definition::{Func, FuncId, FuncInput, FuncOutput};
use crate::node::event::EventLambda;
use crate::testing;

#[test]
fn prune_drops_out_of_range_and_missing_emitter_subscriptions() {
    let func_id = FuncId::from_u128(0xe0e0);
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(func_id, "emitter").event("tick", EventLambda::default()),
    ));

    let mut graph = Graph::default();
    let emitter_id = graph.add(Node::new(NodeKind::Func(func_id)));
    let subscriber_id = graph.add(Node::new(NodeKind::Func(func_id)));
    let missing_emitter = NodeId::unique();
    graph.subscribe(emitter_id, 0, subscriber_id);
    graph.subscribe(emitter_id, 1, subscriber_id);
    graph.subscribe(missing_emitter, 0, subscriber_id);

    graph.prune_dangling_wiring(&library);

    assert!(graph.is_subscribed(emitter_id, 0, subscriber_id));
    assert!(!graph.is_subscribed(emitter_id, 1, subscriber_id));
    assert!(!graph.is_subscribed(missing_emitter, 0, subscriber_id));

    graph.prune_dangling_wiring(&library);
    assert_eq!(graph.subscriptions().count(), 1);

    let unresolved_id = graph.add(Node::new(NodeKind::Func(FuncId::from_u128(0xdead))));
    graph.subscribe(unresolved_id, 4, subscriber_id);
    graph.prune_dangling_wiring(&library);
    assert!(graph.is_subscribed(unresolved_id, 4, subscriber_id));
}

#[test]
fn prune_drops_out_of_range_and_missing_binding_endpoints() {
    let func_id = FuncId::from_u128(0xb12d);
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(func_id, "op")
            .input(FuncInput::optional("in", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int)),
    ));

    let mut graph = Graph::new("test");
    let ids: Vec<NodeId> = (0..5)
        .map(|_| graph.add(Node::new(NodeKind::Func(func_id))))
        .collect();
    let (a, b, c, d, e) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let missing_node = NodeId::unique();
    graph
        .definition
        .as_mut()
        .unwrap()
        .inputs
        .push(FuncInput::optional("in", DataType::Int));
    graph
        .definition
        .as_mut()
        .unwrap()
        .outputs
        .push(FuncOutput::new("out", DataType::Int));
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

    graph.prune_dangling_wiring(&library);

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
    graph.prune_dangling_wiring(&library);
    assert_eq!(
        graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::Const(StaticValue::from(1i64))),
    );

    let unresolved_id = graph.add(Node::new(NodeKind::Func(FuncId::from_u128(0xdead))));
    graph.set_input_binding(InputPort::new(unresolved_id, 3), Binding::bind(a, 0));
    graph.set_input_binding(InputPort::new(b, 0), Binding::bind(unresolved_id, 7));
    graph.prune_dangling_wiring(&library);
    assert_eq!(
        graph.bindings.get(&InputPort::new(unresolved_id, 3)),
        Some(&Binding::bind(a, 0)),
    );
    assert_eq!(
        graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::bind(unresolved_id, 7)),
    );
}
