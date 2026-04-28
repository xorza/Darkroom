use super::*;
use scenarium::graph::{Event, Node, NodeBehavior};

fn empty_graph() -> ViewGraph {
    ViewGraph::default()
}

fn make_node(name: &str) -> (Node, ViewNode) {
    let node = Node {
        id: scenarium::graph::NodeId::unique(),
        name: name.into(),
        func_id: scenarium::function::FuncId::unique(),
        behavior: NodeBehavior::AsFunction,
        inputs: Vec::new(),
        events: Vec::new(),
    };
    let view_node = ViewNode {
        id: node.id,
        pos: egui::Pos2::ZERO,
    };
    (node, view_node)
}

#[test]
#[should_panic(expected = "apply AddNode expects node to be absent")]
fn add_node_apply_panics_on_duplicate() {
    let mut vg = empty_graph();
    let (node, view_node) = make_node("foo");
    let step = UndoStep::AddNode { node, view_node };
    apply_step(&step, &mut vg);
    apply_step(&step, &mut vg);
}

#[test]
#[should_panic(expected = "apply RemoveNode expects node to be present")]
fn remove_node_apply_panics_on_missing() {
    let mut vg = empty_graph();
    let (node, view_node) = make_node("foo");
    let node_id = node.id;
    vg.graph.add(node);
    vg.view_nodes.add(view_node);

    let step = build_step(Intent::RemoveNode { node_id }, &vg);
    apply_step(&step, &mut vg);
    apply_step(&step, &mut vg);
}

#[test]
#[should_panic(expected = "subscriber already present")]
fn set_event_connection_present_panics_on_duplicate() {
    let mut vg = empty_graph();
    let (mut event_node, event_view) = make_node("emitter");
    event_node.events.push(Event {
        subscribers: Vec::new(),
        name: "tick".into(),
    });
    let event_node_id = event_node.id;
    vg.graph.add(event_node);
    vg.view_nodes.add(event_view);

    let (subscriber_node, subscriber_view) = make_node("subscriber");
    let subscriber = subscriber_node.id;
    vg.graph.add(subscriber_node);
    vg.view_nodes.add(subscriber_view);

    let step = build_step(
        Intent::SetEventConnection {
            event_node_id,
            event_idx: 0,
            subscriber,
            present: true,
        },
        &vg,
    );
    apply_step(&step, &mut vg);
    apply_step(&step, &mut vg);
}

#[test]
#[should_panic(expected = "subscriber not present")]
fn set_event_connection_absent_panics_on_missing() {
    let mut vg = empty_graph();
    let (mut event_node, event_view) = make_node("emitter");
    event_node.events.push(Event {
        subscribers: Vec::new(),
        name: "tick".into(),
    });
    let event_node_id = event_node.id;
    vg.graph.add(event_node);
    vg.view_nodes.add(event_view);

    let (subscriber_node, subscriber_view) = make_node("subscriber");
    let subscriber = subscriber_node.id;
    vg.graph.add(subscriber_node);
    vg.view_nodes.add(subscriber_view);

    let step = build_step(
        Intent::SetEventConnection {
            event_node_id,
            event_idx: 0,
            subscriber,
            present: false,
        },
        &vg,
    );
    apply_step(&step, &mut vg);
}
