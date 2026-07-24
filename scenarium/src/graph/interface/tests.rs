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
    graph.definition.as_mut().unwrap().events.push(GraphEvent {
        name: "done".into(),
        emitter,
        emitter_event_idx: 0,
    });
    let instance = Node::graph_instance(&child, GraphLink::Local(child_id));
    graph.insert_graph(child_id, child);
    graph.add(instance);

    let copy = graph.fresh_copy();
    assert_eq!(copy.definition.as_ref().unwrap().origin, None);
    let copied_emitter = copy.definition.as_ref().unwrap().events[0].emitter;
    assert_ne!(copied_emitter, emitter);
    assert!(
        copy.find(&copied_emitter, NodeSearch::TopLevel).is_some(),
        "event emitter follows the copied node"
    );
    assert_eq!(copy.graphs.len(), 1, "the nested def travels with the copy");
    let (copied_child_id, copied_child) = copy.graphs.iter().next().unwrap();
    assert_ne!(
        *copied_child_id, child_id,
        "nested graph identities are remapped"
    );
    assert_eq!(copied_child.definition.as_ref().unwrap().origin, None);
    assert!(
        copied_child
            .find(&child_node, NodeSearch::TopLevel)
            .is_none(),
        "nested node identities are remapped"
    );
    let linked = copy
        .iter()
        .find_map(|n| n.kind.as_graph())
        .expect("instance node copied");
    assert_eq!(
        linked,
        GraphLink::Local(*copied_child_id),
        "the instance's Local link follows the remapped def id"
    );

    // The other copy mode is the exact inverse on every axis `fresh_copy`
    // touches — that contrast is why `Graph` isn't `Clone`.
    let verbatim = graph.verbatim_copy();
    assert_eq!(verbatim, graph, "a verbatim copy is field-for-field equal");
    assert_eq!(
        verbatim.definition.as_ref().unwrap().origin,
        Some(graph_origin),
        "library lineage survives, where fresh_copy clears it"
    );
    assert_eq!(
        verbatim.definition.as_ref().unwrap().events[0].emitter,
        emitter
    );
    let (verbatim_child_id, verbatim_child) = verbatim.graphs.iter().next().unwrap();
    assert_eq!(*verbatim_child_id, child_id, "nested def keeps its id");
    assert_ne!(
        *verbatim_child_id, *copied_child_id,
        "the two copy modes disagree on nested identity"
    );
    assert_eq!(
        verbatim_child.definition.as_ref().unwrap().origin,
        Some(child_origin)
    );
    assert!(
        verbatim_child
            .find(&child_node, NodeSearch::TopLevel)
            .is_some(),
        "nested node ids are preserved, where fresh_copy remaps them"
    );
    assert_eq!(
        verbatim.iter().find_map(|n| n.kind.as_graph()),
        Some(GraphLink::Local(child_id)),
        "the instance's Local link still names the original def"
    );
}
