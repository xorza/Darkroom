use crate::graph::interface::{GraphEvent, GraphId, GraphLink};
use crate::graph::{Graph, GraphDef, Node, NodeKind, NodeSearch};
use crate::node::definition::FuncId;

/// The link of the one graph-instance node `graph` holds.
fn instance_link(graph: &Graph) -> Option<GraphLink> {
    graph.iter().find_map(|node| match node.kind {
        NodeKind::Graph(link) => Some(link),
        _ => None,
    })
}

#[test]
fn graph_link_preserves_registry_and_identity() {
    let id = GraphId::unique();
    assert_eq!(GraphLink::Local(id).id(), id);
    assert_eq!(GraphLink::Shared(id).id(), id);
    assert_ne!(GraphLink::Local(id), GraphLink::Shared(id));
}

#[test]
fn clone_mapped_remaps_nodes_events_and_nested_graphs() {
    let child_id = GraphId::unique();
    let child_origin = GraphId::unique();
    let mut child = GraphDef::new("child").origin(child_origin);
    let child_node = child.body.add(Node::new(NodeKind::Func(FuncId::unique())));

    let graph_origin = GraphId::unique();
    let mut graph = GraphDef::new("parent").origin(graph_origin);
    let emitter = graph.body.add(Node::new(NodeKind::Func(FuncId::unique())));
    graph.definition.events.push(GraphEvent {
        name: "done".into(),
        emitter,
        emitter_event_idx: 0,
    });
    let instance = Node::graph_instance(&child, GraphLink::Local(child_id));
    graph.body.insert_graph(child_id, child);
    graph.body.add(instance);

    let copy = graph.clone_mapped();
    assert_eq!(copy.definition.origin, None);
    let copied_emitter = copy.definition.events[0].emitter;
    assert_ne!(copied_emitter, emitter);
    assert!(
        copy.body
            .find(copied_emitter, NodeSearch::TopLevel)
            .is_some(),
        "event emitter follows the copied node"
    );
    assert_eq!(
        copy.body.graphs.len(),
        1,
        "the nested def travels with the copy"
    );
    let (copied_child_id, copied_child) = copy.body.graphs.iter().next().unwrap();
    assert_ne!(
        *copied_child_id, child_id,
        "nested graph identities are remapped"
    );
    assert_eq!(copied_child.definition.origin, None);
    assert!(
        copied_child
            .body
            .find(child_node, NodeSearch::TopLevel)
            .is_none(),
        "nested node identities are remapped"
    );
    let linked = instance_link(&copy.body).expect("instance node copied");
    assert_eq!(
        linked,
        GraphLink::Local(*copied_child_id),
        "the instance's Local link follows the remapped def id"
    );

    // The other copy mode is the exact inverse on every axis `clone_mapped`
    // touches — that contrast is why `Graph` isn't `Clone`.
    let verbatim = graph.clone_verbatim();
    assert_eq!(verbatim, graph, "a verbatim copy is field-for-field equal");
    assert_eq!(
        verbatim.definition.origin,
        Some(graph_origin),
        "library lineage survives, where clone_mapped clears it"
    );
    assert_eq!(verbatim.definition.events[0].emitter, emitter);
    let (verbatim_child_id, verbatim_child) = verbatim.body.graphs.iter().next().unwrap();
    assert_eq!(*verbatim_child_id, child_id, "nested def keeps its id");
    assert_ne!(
        *verbatim_child_id, *copied_child_id,
        "the two copy modes disagree on nested identity"
    );
    assert_eq!(verbatim_child.definition.origin, Some(child_origin));
    assert!(
        verbatim_child
            .body
            .find(child_node, NodeSearch::TopLevel)
            .is_some(),
        "nested node ids are preserved, where clone_mapped remaps them"
    );
    assert_eq!(
        instance_link(&verbatim.body),
        Some(GraphLink::Local(child_id)),
        "the instance's Local link still names the original def"
    );
}
