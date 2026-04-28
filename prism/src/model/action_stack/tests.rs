use super::*;
use crate::common::UiEquals;
use crate::model::ViewNode;
use crate::model::intent::{self, Intent};
use common::SerdeFormat;
use egui::{Vec2, vec2};
use scenarium::data::StaticValue;
use scenarium::graph::{Binding, Event, Input, NodeBehavior};
use scenarium::testing::test_graph;

fn assert_ranges_match_actions(stack: &ActionStack) {
    for entry in &stack.undo_stack {
        assert!(entry.range.start <= entry.range.end);
        assert!(entry.range.end <= stack.undo_actions.len());
    }
    for range in &stack.redo_stack {
        assert!(range.start <= range.end);
        assert!(range.end <= stack.redo_actions.len());
    }

    let undo_actions_len: usize = stack
        .undo_stack
        .iter()
        .map(|entry| entry.range.end - entry.range.start)
        .sum();
    let redo_actions_len: usize = stack
        .redo_stack
        .iter()
        .map(|range| range.end - range.start)
        .sum();

    assert_eq!(undo_actions_len, stack.undo_actions.len());
    assert_eq!(redo_actions_len, stack.redo_actions.len());
}

/// Apply an intent to `view_graph` and return the matching `UndoStep`
/// (capture-then-apply). Mirrors what `Session::commit_action_slice`
/// does in production code.
fn step(view_graph: &mut ViewGraph, intent: Intent) -> UndoStep {
    let snapshot = intent::capture(&intent, view_graph);
    intent::apply(&intent, view_graph);
    UndoStep { intent, snapshot }
}

#[test]
fn max_steps_must_be_positive() {
    let result = std::panic::catch_unwind(|| ActionStack::new(0));
    assert!(result.is_err());
}

#[test]
fn intent_undo_redo_roundtrip() {
    let graph = test_graph();
    let mut view_graph: ViewGraph = graph.into();
    let original = view_graph.serialize(SerdeFormat::Json);

    let node_id = view_graph.graph.iter().next().unwrap().id;
    let before_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;
    let after_pos = before_pos + vec2(10.0, 5.0);

    let steps = vec![
        step(
            &mut view_graph,
            Intent::MoveNode {
                node_id,
                to: after_pos,
            },
        ),
        step(&mut view_graph, Intent::SelectNode { to: Some(node_id) }),
        step(
            &mut view_graph,
            Intent::SetViewport {
                pan: vec2(20.0, -10.0),
                scale: 1.5,
            },
        ),
    ];
    let modified = view_graph.serialize(SerdeFormat::Json);

    let mut stack = ActionStack::new(16);
    stack.clear();
    stack.push_current(&steps);

    assert_ranges_match_actions(&stack);
    assert!(stack.undo(&mut view_graph, &mut |_| {}));
    assert_ranges_match_actions(&stack);
    assert_eq!(view_graph.serialize(SerdeFormat::Json), original);

    assert!(stack.redo(&mut view_graph, &mut |_| {}));
    assert_ranges_match_actions(&stack);
    assert_eq!(view_graph.serialize(SerdeFormat::Json), modified);
}

#[test]
fn max_steps_limits_history() {
    let graph = test_graph();
    let mut view_graph: ViewGraph = graph.into();
    let node_id = view_graph.graph.iter().next().unwrap().id;
    let before_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;

    let mut stack = ActionStack::new(1);

    let s1 = step(
        &mut view_graph,
        Intent::MoveNode {
            node_id,
            to: before_pos + vec2(1.0, 0.0),
        },
    );
    stack.push_current(&[s1]);
    assert_ranges_match_actions(&stack);

    let s2 = step(
        &mut view_graph,
        Intent::MoveNode {
            node_id,
            to: before_pos + vec2(2.0, 0.0),
        },
    );
    stack.push_current(&[s2]);
    assert_ranges_match_actions(&stack);

    assert!(stack.undo(&mut view_graph, &mut |_| {}));
    assert_ranges_match_actions(&stack);
    assert_eq!(
        view_graph.view_nodes.by_key(&node_id).unwrap().pos,
        before_pos + vec2(1.0, 0.0)
    );
    assert!(!stack.undo(&mut view_graph, &mut |_| {}));
    assert_ranges_match_actions(&stack);
}

/// After many pushes past `max_steps`, the undo buffer must stay
/// bounded — the drained prefix is not supposed to accumulate.
#[test]
fn undo_buffer_stays_bounded_past_max_steps() {
    let graph = test_graph();
    let mut view_graph: ViewGraph = graph.into();
    let node_id = view_graph.graph.iter().next().unwrap().id;
    let start_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;
    let mut stack = ActionStack::new(2);
    stack.clear();

    for i in 0..20 {
        let s = step(
            &mut view_graph,
            Intent::MoveNode {
                node_id,
                to: start_pos + vec2((i + 1) as f32, 0.0),
            },
        );
        stack.push_current(&[s]);
        assert_ranges_match_actions(&stack);
        assert!(stack.undo_stack.len() <= 2);
    }

    let total: usize = stack
        .undo_stack
        .iter()
        .map(|e| e.range.end - e.range.start)
        .sum();
    assert_eq!(total, stack.undo_actions.len());
}

#[test]
fn undo_roundtrip_all_intent_variants_with_json_snapshots() {
    let graph = test_graph();
    let mut view_graph: ViewGraph = graph.into();
    let mut stack = ActionStack::new(32);

    let node_ids: Vec<_> = view_graph.graph.iter().map(|node| node.id).collect();
    let primary_id = *node_ids.first().expect("test graph must have nodes");
    let secondary_id = node_ids
        .iter()
        .copied()
        .find(|node_id| *node_id != primary_id)
        .unwrap_or(primary_id);

    if view_graph.graph.iter().all(|node| node.events.is_empty()) {
        view_graph
            .graph
            .by_id_mut(&primary_id)
            .unwrap()
            .events
            .push(Event {
                subscribers: Vec::new(),
                name: "input a".into(),
            });
    }
    if view_graph.graph.iter().all(|node| node.inputs.is_empty()) {
        view_graph
            .graph
            .by_id_mut(&primary_id)
            .unwrap()
            .inputs
            .push(Input {
                binding: Binding::None,
                name: "input a".into(),
            });
    }
    let input_node_id = view_graph
        .graph
        .iter()
        .find(|node| !node.inputs.is_empty())
        .map(|node| node.id)
        .expect("test graph must include input nodes");
    let input_idx = 0;
    view_graph.graph.by_id_mut(&input_node_id).unwrap().inputs[input_idx].binding = Binding::None;

    stack.clear();
    let mut snapshots = vec![view_graph.serialize(SerdeFormat::Json)];

    let cache_before = view_graph.graph.by_id(&primary_id).unwrap().behavior;
    let cache_after = match cache_before {
        NodeBehavior::AsFunction => NodeBehavior::Once,
        NodeBehavior::Once => NodeBehavior::AsFunction,
    };
    let s = step(
        &mut view_graph,
        Intent::SetCacheBehavior {
            node_id: primary_id,
            to: cache_after,
        },
    );
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    let (event_node_id, event_idx, subscriber, present) = {
        let event_node = view_graph
            .graph
            .iter()
            .find(|node| !node.events.is_empty())
            .expect("test graph must include event nodes");
        let event_idx = 0;
        let subscribers = &event_node.events[event_idx].subscribers;
        let (subscriber, present) = if let Some(existing) = subscribers.first() {
            (*existing, false) // remove existing
        } else {
            let subscriber = node_ids
                .iter()
                .copied()
                .find(|node_id| *node_id != event_node.id)
                .unwrap_or(event_node.id);
            (subscriber, true) // add new
        };
        (event_node.id, event_idx, subscriber, present)
    };
    let s = step(
        &mut view_graph,
        Intent::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
        },
    );
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    let s = step(
        &mut view_graph,
        Intent::SetInput {
            node_id: input_node_id,
            input_idx,
            to: Binding::Const(StaticValue::Int(123)),
        },
    );
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    let moved_before = view_graph.view_nodes.by_key(&primary_id).unwrap().pos;
    let s = step(
        &mut view_graph,
        Intent::MoveNode {
            node_id: primary_id,
            to: moved_before + vec2(5.0, -3.0),
        },
    );
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    let selected_before = view_graph.selected_node_id;
    let selected_after = match selected_before {
        Some(id) if id == primary_id => None,
        _ => Some(primary_id),
    };
    let s = step(&mut view_graph, Intent::SelectNode { to: selected_after });
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    let zoom_before_pan = view_graph.pan;
    let zoom_before_scale = view_graph.scale;
    let s = step(
        &mut view_graph,
        Intent::SetViewport {
            pan: zoom_before_pan + vec2(12.0, -6.0),
            scale: zoom_before_scale + 0.25,
        },
    );
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    let mut bound_targets = std::collections::HashSet::new();
    for node in view_graph.graph.iter() {
        for input in &node.inputs {
            if let Binding::Bind(address) = &input.binding {
                bound_targets.insert(address.target_id);
            }
        }
    }
    let removed_node_id = node_ids
        .iter()
        .copied()
        .find(|node_id| !bound_targets.contains(node_id))
        .unwrap_or(secondary_id);
    let s = step(
        &mut view_graph,
        Intent::RemoveNode {
            node_id: removed_node_id,
        },
    );
    stack.push_current(std::slice::from_ref(&s));
    snapshots.push(view_graph.serialize(SerdeFormat::Json));

    for snapshot_idx in (1..snapshots.len()).rev() {
        assert!(stack.undo(&mut view_graph, &mut |_| {}));
        let snapshot_graph =
            ViewGraph::deserialize(SerdeFormat::Json, &snapshots[snapshot_idx - 1])
                .expect("snapshot should deserialize");
        assert_eq!(view_graph.graph, snapshot_graph.graph);
        assert_eq!(view_graph.view_nodes, snapshot_graph.view_nodes);
        assert!(view_graph.pan.ui_equals(snapshot_graph.pan));
        assert!(view_graph.scale.ui_equals(snapshot_graph.scale));
        assert_eq!(view_graph.selected_node_id, snapshot_graph.selected_node_id);
    }
    assert!(!stack.undo(&mut view_graph, &mut |_| {}));
}

/// Continuous SetViewport emissions merge into a single undo entry.
/// The merged step keeps the *first* snapshot (gesture start) and
/// the *latest* intent (gesture end).
#[test]
fn viewport_merges_into_single_undo_entry() {
    let mut vg = ViewGraph::default();
    let mut stack = ActionStack::new(32);
    stack.clear();

    let frames = [
        (Vec2::new(10.0, 0.0), 1.1),
        (Vec2::new(25.0, -5.0), 1.25),
        (Vec2::new(40.0, -10.0), 1.4),
    ];
    for (pan, scale) in frames {
        let s = step(&mut vg, Intent::SetViewport { pan, scale });
        stack.push_current(&[s]);
    }

    assert_eq!(
        stack.undo_stack.len(),
        1,
        "consecutive SetViewport must merge into one undo entry"
    );

    assert!(stack.undo(&mut vg, &mut |_| {}));
    assert!(vg.pan.ui_equals(Vec2::ZERO));
    assert!(vg.scale.ui_equals(1.0));
}

/// Merge only spans consecutive gestures of the same kind. Any
/// other action between two SetViewports breaks the merge.
#[test]
fn viewport_merge_does_not_span_other_actions() {
    use egui::Pos2;

    let node = scenarium::graph::Node {
        id: scenarium::graph::NodeId::unique(),
        func_id: scenarium::function::FuncId::unique(),
        name: String::new(),
        behavior: scenarium::graph::NodeBehavior::AsFunction,
        inputs: Vec::new(),
        events: Vec::new(),
    };
    let view_node = ViewNode {
        id: node.id,
        pos: Pos2::ZERO,
    };

    let mut vg = ViewGraph::default();
    let mut stack = ActionStack::new(32);
    stack.clear();

    let s = step(
        &mut vg,
        Intent::SetViewport {
            pan: Vec2::new(10.0, 0.0),
            scale: 1.1,
        },
    );
    stack.push_current(&[s]);

    let s = step(
        &mut vg,
        Intent::AddNode {
            view_node: view_node.clone(),
            node: node.clone(),
        },
    );
    stack.push_current(&[s]);

    let s = step(
        &mut vg,
        Intent::SetViewport {
            pan: Vec2::new(20.0, 0.0),
            scale: 1.2,
        },
    );
    stack.push_current(&[s]);

    assert_eq!(
        stack.undo_stack.len(),
        3,
        "AddNode between viewport changes must prevent merge"
    );
}
