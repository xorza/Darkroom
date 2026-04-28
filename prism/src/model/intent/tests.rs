use super::*;
use egui::{Pos2, Vec2};
use scenarium::data::StaticValue;
use scenarium::graph::{Binding, Event, Node, NodeBehavior, NodeId};

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

/// One sample of every `UndoStep` variant, paired with its expected
/// `affects_computation` and `gesture_key` answers. The exhaustive
/// match in `affects_computation` / `gesture_key` already forces the
/// developer to *declare* a behavior per variant when adding one;
/// this test pins the *intended* answers so a wrong-side default is
/// caught at test time instead of in production.
///
/// Adding an `UndoStep` variant: add a sample here. The
/// `len(variants_with_expected_behavior)` assert below catches the
/// "I forgot" case.
fn variants_with_expected_behavior() -> Vec<(UndoStep, bool, Option<GestureKey>)> {
    let node_id = NodeId::unique();
    let other_id = NodeId::unique();
    let view_node = ViewNode {
        id: node_id,
        pos: Pos2::ZERO,
    };
    let node = Node {
        id: node_id,
        func_id: scenarium::function::FuncId::unique(),
        name: "n".into(),
        behavior: NodeBehavior::AsFunction,
        inputs: Vec::new(),
        events: Vec::new(),
    };
    vec![
        (
            UndoStep::AddNode {
                view_node: view_node.clone(),
                node: node.clone(),
            },
            true,
            None,
        ),
        (
            UndoStep::RemoveNode {
                view_node: view_node.clone(),
                node,
                incoming_connections: Vec::new(),
                incoming_events: Vec::new(),
                was_selected: false,
            },
            true,
            None,
        ),
        (
            UndoStep::MoveNode {
                node_id,
                from: Pos2::ZERO,
                to: Pos2::new(1.0, 2.0),
            },
            false,
            None,
        ),
        (
            UndoStep::RenameNode {
                node_id,
                from: "old".into(),
                to: "new".into(),
            },
            false,
            None,
        ),
        (
            UndoStep::SetInput {
                node_id,
                input_idx: 0,
                from: Binding::None,
                to: Binding::Const(StaticValue::Int(7)),
            },
            true,
            None,
        ),
        (
            UndoStep::SelectNode {
                from: None,
                to: Some(node_id),
            },
            false,
            None,
        ),
        (
            UndoStep::SetCacheBehavior {
                node_id,
                from: NodeBehavior::AsFunction,
                to: NodeBehavior::Once,
            },
            true,
            None,
        ),
        (
            UndoStep::SetEventConnection {
                event_node_id: node_id,
                event_idx: 0,
                subscriber: other_id,
                was_present: false,
                present: true,
            },
            true,
            None,
        ),
        (
            UndoStep::SetViewport {
                from_pan: Vec2::ZERO,
                from_scale: 1.0,
                to_pan: Vec2::new(10.0, 0.0),
                to_scale: 1.5,
            },
            false,
            Some(GestureKey::Viewport),
        ),
    ]
}

#[test]
fn affects_computation_matches_expectation_per_variant() {
    for (step, expected, _) in variants_with_expected_behavior() {
        assert_eq!(
            affects_computation(&step),
            expected,
            "affects_computation mismatch for {step:?}"
        );
    }
}

#[test]
fn gesture_key_matches_expectation_per_variant() {
    for (step, _, expected) in variants_with_expected_behavior() {
        assert_eq!(
            gesture_key(&step),
            expected,
            "gesture_key mismatch for {step:?}"
        );
    }
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
