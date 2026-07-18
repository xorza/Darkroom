use crate::graph::interface::{GraphEvent, GraphId, GraphLink};
use crate::graph::{Graph, Node, NodeKind, NodeSearch};
use crate::node::definition::FuncId;

#[test]
fn graph_link_preserves_registry_and_identity() {
    let id = GraphId::unique();
    assert_eq!(GraphLink::Local(id).id(), id);
    assert_eq!(GraphLink::Shared(id).id(), id);
    assert_ne!(GraphLink::Local(id), GraphLink::Shared(id));
}

#[test]
fn fresh_copy_remaps_nodes_events_and_nested_graphs() {
    let child_id = GraphId::unique();
    let child_origin = GraphId::unique();
    let mut child = Graph::new("child").origin(child_origin);
    let child_node = child.add(Node::new(NodeKind::Func(FuncId::unique())));

    let graph_origin = GraphId::unique();
    let mut graph = Graph::new("parent").origin(graph_origin);
    let emitter = graph.add(Node::new(NodeKind::Func(FuncId::unique())));
    graph.events.push(GraphEvent {
        name: "done".into(),
        emitter,
        emitter_event_idx: 0,
    });
    graph.insert_graph(child_id, child);

    let copy = graph.fresh_copy();
    assert_eq!(copy.origin, None);
    let copied_emitter = copy.events[0].emitter;
    assert_ne!(copied_emitter, emitter);
    assert!(
        copy.find(&copied_emitter, NodeSearch::TopLevel).is_some(),
        "event emitter follows the copied node"
    );
    let copied_child = &copy.graphs[&child_id];
    assert_eq!(copied_child.origin, None);
    assert!(
        copied_child
            .find(&child_node, NodeSearch::TopLevel)
            .is_none(),
        "nested node identities are remapped"
    );
    assert_eq!(copy.graphs.len(), 1, "nested graph identity is the map key");
}
