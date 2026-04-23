use std::fmt::Debug;
use std::ops::Range;

use common::SerdeFormat;

use crate::model::ViewGraph;
use crate::model::graph_ui_action::{GestureKey, GraphUiAction};

#[derive(Debug)]
struct UndoEntry {
    range: Range<usize>,
    // Cached so gesture-merge can reject without deserializing the entry.
    // Only set for single-action batches that identify as a gesture.
    gesture_key: Option<GestureKey>,
}

/// Undo history kept as two flat byte buffers with per-entry ranges,
/// rather than `VecDeque<Vec<GraphUiAction>>`.
///
/// Why: `GraphUiAction::NodeRemoved` carries a full `Node` + per-edge
/// `Vec<IncomingConnection>` / `Vec<IncomingEvent>`; a naive enum-storage
/// history hits the allocator once per field per pushed action and leaves
/// the heap fragmented as the ring trims old entries. Bitcode-packing
/// into one contiguous buffer keeps the whole history in two allocations
/// (undo + redo) regardless of entry count, makes `trim_to_limit` a
/// single `Vec::drain` of the packed prefix, and keeps cache locality
/// when we walk the stack. The bitcode encode/decode cost is bounded
/// (one batch per push, undo, or redo — not per frame).
///
/// Consequences to respect:
/// - `undo_stack[i].range.end <= undo_actions.len()` is a non-local
///   invariant; `assert_ranges_match_actions` in tests watches for drift.
/// - Gesture-merge deserializes the tail entry, edits, re-serializes.
///   O(1) check via the cached `gesture_key` avoids that on the miss path.
#[derive(Debug)]
pub struct ActionStack {
    undo_actions: Vec<u8>,
    redo_actions: Vec<u8>,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<Range<usize>>,
    max_steps: usize,

    temp_buffer: Vec<u8>,
}

impl ActionStack {
    pub fn new(max_steps: usize) -> Self {
        assert!(max_steps > 0, "undo stack must allow at least one step");
        Self {
            undo_actions: Vec::new(),
            redo_actions: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_steps,
            temp_buffer: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.undo_actions.clear();
        self.redo_actions.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn push_current(&mut self, actions: &[GraphUiAction]) {
        if actions.is_empty() {
            return;
        }
        self.redo_actions.clear();
        self.redo_stack.clear();

        // Gesture coalescing: if this push is a single action matching
        // the previous entry's cached gesture_key, merge in place.
        // Cross-frame zoom/pan collapses to one undo step without any
        // pending-action state outside the stack (important under
        // egui multi-pass).
        if actions.len() == 1
            && let Some(key) = actions[0].gesture_key()
            && self.try_merge_with_last(&actions[0], key)
        {
            return;
        }

        let range = Self::append_actions(&mut self.undo_actions, actions, &mut self.temp_buffer);
        let gesture_key = if actions.len() == 1 {
            actions[0].gesture_key()
        } else {
            None
        };
        self.undo_stack.push(UndoEntry { range, gesture_key });
        self.trim_to_limit();
    }

    pub fn clear_redo(&mut self) {
        self.redo_actions.clear();
        self.redo_stack.clear();
    }

    pub fn undo(
        &mut self,
        value: &mut ViewGraph,
        on_action: &mut dyn FnMut(&GraphUiAction),
    ) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        let bytes = Self::slice_bytes(&self.undo_actions, &entry.range);
        let actions = Self::deserialize_actions(bytes, &mut self.temp_buffer);
        for action in actions.iter().rev() {
            action.undo(value);
            on_action(action);
        }
        let redo_range = Self::append_bytes(&mut self.redo_actions, bytes);
        self.redo_stack.push(redo_range);
        Self::pop_tail_actions(&mut self.undo_actions, &entry.range);

        true
    }

    pub fn redo(
        &mut self,
        value: &mut ViewGraph,
        on_action: &mut dyn FnMut(&GraphUiAction),
    ) -> bool {
        let Some(range) = self.redo_stack.pop() else {
            return false;
        };
        let bytes = Self::slice_bytes(&self.redo_actions, &range);
        let actions = Self::deserialize_actions(bytes, &mut self.temp_buffer);
        let gesture_key = if actions.len() == 1 {
            actions[0].gesture_key()
        } else {
            None
        };
        for action in actions.iter() {
            action.apply(value);
            on_action(action);
        }
        let undo_range = Self::append_bytes(&mut self.undo_actions, bytes);
        self.undo_stack.push(UndoEntry {
            range: undo_range,
            gesture_key,
        });
        Self::pop_tail_actions(&mut self.redo_actions, &range);

        true
    }

    fn trim_to_limit(&mut self) {
        while self.undo_stack.len() > self.max_steps {
            let removed = self.undo_stack.remove(0);
            // Drain the dropped prefix immediately and renormalize the
            // remaining ranges. max_steps is small (~100), so this is
            // cheap and saves carrying a base_offset field.
            let drop_end = removed.range.end;
            self.undo_actions.drain(0..drop_end);
            for entry in &mut self.undo_stack {
                entry.range.start -= drop_end;
                entry.range.end -= drop_end;
            }
        }
    }

    fn try_merge_with_last(&mut self, new_action: &GraphUiAction, key: GestureKey) -> bool {
        let Some(last) = self.undo_stack.last() else {
            return false;
        };
        if last.gesture_key != Some(key) {
            return false;
        }
        let last_range = last.range.clone();
        let last_bytes = Self::slice_bytes(&self.undo_actions, &last_range);
        let last_actions = Self::deserialize_actions(last_bytes, &mut self.temp_buffer);
        assert_eq!(
            last_actions.len(),
            1,
            "gesture-keyed entry must hold a single action"
        );
        let Some(merged) = last_actions[0].merge(new_action) else {
            return false;
        };

        Self::pop_tail_actions(&mut self.undo_actions, &last_range);
        self.undo_stack.pop();
        let range = Self::append_actions(&mut self.undo_actions, &[merged], &mut self.temp_buffer);
        self.undo_stack.push(UndoEntry {
            range,
            gesture_key: Some(key),
        });
        true
    }

    fn append_actions(
        buffer: &mut Vec<u8>,
        actions: &[GraphUiAction],
        temp_buffer: &mut Vec<u8>,
    ) -> Range<usize> {
        assert!(
            !actions.is_empty(),
            "undo stack should not store empty action batches"
        );
        let start = buffer.len();
        common::serde::serialize_into(actions, SerdeFormat::Bitcode, buffer, temp_buffer);
        let end = buffer.len();
        start..end
    }

    fn deserialize_actions(bytes: &[u8], temp_buffer: &mut Vec<u8>) -> Vec<GraphUiAction> {
        common::serde::deserialize_from(
            &mut std::io::Cursor::new(bytes),
            SerdeFormat::Bitcode,
            temp_buffer,
        )
        .unwrap()
    }

    fn append_bytes(target: &mut Vec<u8>, bytes: &[u8]) -> Range<usize> {
        let start = target.len();
        target.extend_from_slice(bytes);
        let end = target.len();
        start..end
    }

    fn slice_bytes<'a>(buffer: &'a [u8], range: &Range<usize>) -> &'a [u8] {
        assert!(range.start <= range.end, "undo stack range start > end");
        assert!(
            range.end <= buffer.len(),
            "undo stack range exceeds buffer length"
        );
        &buffer[range.clone()]
    }

    fn pop_tail_actions(buffer: &mut Vec<u8>, range: &Range<usize>) {
        if range.end == buffer.len() {
            buffer.truncate(range.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::UiEquals;
    use crate::model::EventSubscriberChange;
    use crate::model::graph_ui_action::GraphUiAction;
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

    #[test]
    fn max_steps_must_be_positive() {
        let result = std::panic::catch_unwind(|| ActionStack::new(0));
        assert!(result.is_err());
    }

    #[test]
    fn action_undo_redo_roundtrip() {
        let graph = test_graph();
        let mut view_graph: ViewGraph = graph.into();
        let original = view_graph.serialize(SerdeFormat::Json);

        let node_id = view_graph.graph.iter().next().unwrap().id;
        let before_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;
        let after_pos = before_pos + vec2(10.0, 5.0);

        let actions = vec![
            GraphUiAction::NodeMoved {
                node_id,
                before: before_pos,
                after: after_pos,
            },
            GraphUiAction::NodeSelected {
                before: None,
                after: Some(node_id),
            },
            GraphUiAction::ZoomPanChanged {
                before_pan: Vec2::ZERO,
                before_scale: 1.0,
                after_pan: vec2(20.0, -10.0),
                after_scale: 1.5,
            },
        ];

        let mut stack = ActionStack::new(16);
        stack.clear();

        for action in &actions {
            action.apply(&mut view_graph);
        }
        let modified = view_graph.serialize(SerdeFormat::Json);

        stack.push_current(&actions);

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

        let first_actions = vec![GraphUiAction::NodeMoved {
            node_id,
            before: before_pos,
            after: before_pos + vec2(1.0, 0.0),
        }];
        for action in &first_actions {
            action.apply(&mut view_graph);
        }
        stack.push_current(&first_actions);
        assert_ranges_match_actions(&stack);

        let second_actions = vec![GraphUiAction::NodeMoved {
            node_id,
            before: before_pos + vec2(1.0, 0.0),
            after: before_pos + vec2(2.0, 0.0),
        }];
        for action in &second_actions {
            action.apply(&mut view_graph);
        }
        stack.push_current(&second_actions);
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
        let view_graph: ViewGraph = graph.into();
        let node_id = view_graph.graph.iter().next().unwrap().id;
        let start_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;
        let mut stack = ActionStack::new(2);

        stack.clear();

        for i in 0..20 {
            let action = GraphUiAction::NodeMoved {
                node_id,
                before: start_pos + vec2(i as f32, 0.0),
                after: start_pos + vec2((i + 1) as f32, 0.0),
            };
            stack.push_current(&[action]);
            assert_ranges_match_actions(&stack);
            assert!(stack.undo_stack.len() <= 2);
        }

        // Sum of ranges must equal total buffer length — no orphan bytes.
        let total: usize = stack
            .undo_stack
            .iter()
            .map(|e| e.range.end - e.range.start)
            .sum();
        assert_eq!(total, stack.undo_actions.len());
    }

    #[test]
    fn undo_roundtrip_all_action_variants_with_json_snapshots() {
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
            let node = view_graph
                .graph
                .by_id_mut(&primary_id)
                .expect("primary node must exist");
            node.events.push(Event {
                subscribers: Vec::new(),
                name: "input a".into(),
            });
        }
        if view_graph.graph.iter().all(|node| node.inputs.is_empty()) {
            let node = view_graph
                .graph
                .by_id_mut(&primary_id)
                .expect("primary node must exist");
            node.inputs.push(Input {
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
        {
            let node = view_graph
                .graph
                .by_id_mut(&input_node_id)
                .expect("input node must exist");
            node.inputs[input_idx].binding = Binding::None;
        }

        stack.clear();
        let mut snapshots = vec![view_graph.serialize(SerdeFormat::Json)];

        let cache_before = view_graph.graph.by_id(&primary_id).unwrap().behavior;
        let cache_after = match cache_before {
            NodeBehavior::AsFunction => NodeBehavior::Once,
            NodeBehavior::Once => NodeBehavior::AsFunction,
        };
        let action = GraphUiAction::CacheToggled {
            node_id: primary_id,
            before: cache_before,
            after: cache_after,
        };
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(SerdeFormat::Json));

        let event_node = view_graph
            .graph
            .iter()
            .find(|node| !node.events.is_empty())
            .expect("test graph must include event nodes");
        let event_idx = 0;
        let subscribers = &event_node.events[event_idx].subscribers;
        let (subscriber, change) = if let Some(existing) = subscribers.first() {
            (*existing, EventSubscriberChange::Removed)
        } else {
            let subscriber = node_ids
                .iter()
                .copied()
                .find(|node_id| *node_id != event_node.id)
                .unwrap_or(event_node.id);
            (subscriber, EventSubscriberChange::Added)
        };
        let action = GraphUiAction::EventConnectionChanged {
            event_node_id: event_node.id,
            event_idx,
            subscriber,
            change,
        };
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(SerdeFormat::Json));

        let before_binding = Binding::None;
        let after_binding = Binding::Const(StaticValue::Int(123));
        let action = GraphUiAction::InputChanged {
            node_id: input_node_id,
            input_idx,
            before: before_binding,
            after: after_binding,
        };
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(SerdeFormat::Json));

        let moved_before = view_graph.view_nodes.by_key(&primary_id).unwrap().pos;
        let moved_after = moved_before + vec2(5.0, -3.0);
        let action = GraphUiAction::NodeMoved {
            node_id: primary_id,
            before: moved_before,
            after: moved_after,
        };
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(SerdeFormat::Json));

        let selected_before = view_graph.selected_node_id;
        let selected_after = match selected_before {
            Some(id) if id == primary_id => None,
            _ => Some(primary_id),
        };
        let action = GraphUiAction::NodeSelected {
            before: selected_before,
            after: selected_after,
        };
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(SerdeFormat::Json));

        let zoom_before_pan = view_graph.pan;
        let zoom_before_scale = view_graph.scale;
        let action = GraphUiAction::ZoomPanChanged {
            before_pan: zoom_before_pan,
            before_scale: zoom_before_scale,
            after_pan: zoom_before_pan + vec2(12.0, -6.0),
            after_scale: zoom_before_scale + 0.25,
        };
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
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
        let action = GraphUiAction::node_removal(&view_graph, &removed_node_id);
        action.apply(&mut view_graph);
        stack.push_current(std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(SerdeFormat::Json));

        for (entry_idx, entry) in stack.undo_stack.iter().enumerate() {
            let bytes = ActionStack::slice_bytes(&stack.undo_actions, &entry.range);
            let decoded: Vec<GraphUiAction> =
                common::serde::deserialize(bytes, SerdeFormat::Bitcode).unwrap_or_else(|err| {
                    panic!("undo entry {} failed to deserialize: {}", entry_idx, err)
                });
            assert_eq!(decoded.len(), 1);
        }

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

    /// Continuous ZoomPanChanged emissions merge into a single undo
    /// entry — one gesture, one undo step. Cached gesture_key makes
    /// the merge check O(1) (no deserialize).
    #[test]
    fn zoom_pan_changed_merges_into_single_undo_entry() {
        use egui::Vec2;

        let mut vg = ViewGraph::default();
        let mut stack = ActionStack::new(32);
        stack.clear();

        let frame_deltas = [
            (Vec2::ZERO, 1.0, Vec2::new(10.0, 0.0), 1.1),
            (Vec2::new(10.0, 0.0), 1.1, Vec2::new(25.0, -5.0), 1.25),
            (Vec2::new(25.0, -5.0), 1.25, Vec2::new(40.0, -10.0), 1.4),
        ];
        for (before_pan, before_scale, after_pan, after_scale) in frame_deltas {
            let action = GraphUiAction::ZoomPanChanged {
                before_pan,
                before_scale,
                after_pan,
                after_scale,
            };
            action.apply(&mut vg);
            stack.push_current(std::slice::from_ref(&action));
        }

        assert_eq!(
            stack.undo_stack.len(),
            1,
            "three consecutive ZoomPanChanged emissions must merge into one undo entry"
        );

        assert!(stack.undo(&mut vg, &mut |_| {}));
        assert!(vg.pan.ui_equals(Vec2::ZERO));
        assert!(vg.scale.ui_equals(1.0));
    }

    /// Merging only happens between ZoomPanChanged on ZoomPanChanged —
    /// any other action in between creates a new undo entry.
    #[test]
    fn zoom_pan_merge_does_not_span_other_actions() {
        use egui::{Pos2, Vec2};

        let node = scenarium::graph::Node {
            id: scenarium::graph::NodeId::unique(),
            func_id: scenarium::function::FuncId::unique(),
            name: String::new(),
            behavior: scenarium::graph::NodeBehavior::AsFunction,
            inputs: Vec::new(),
            events: Vec::new(),
        };
        let view_node = crate::model::ViewNode {
            id: node.id,
            pos: Pos2::ZERO,
        };

        let mut vg = ViewGraph::default();
        let mut stack = ActionStack::new(32);
        stack.clear();

        let zoom = GraphUiAction::ZoomPanChanged {
            before_pan: Vec2::ZERO,
            before_scale: 1.0,
            after_pan: Vec2::new(10.0, 0.0),
            after_scale: 1.1,
        };
        zoom.apply(&mut vg);
        stack.push_current(std::slice::from_ref(&zoom));

        let add = GraphUiAction::NodeAdded {
            view_node: view_node.clone(),
            node: node.clone(),
        };
        add.apply(&mut vg);
        stack.push_current(std::slice::from_ref(&add));

        let zoom2 = GraphUiAction::ZoomPanChanged {
            before_pan: Vec2::new(10.0, 0.0),
            before_scale: 1.1,
            after_pan: Vec2::new(20.0, 0.0),
            after_scale: 1.2,
        };
        zoom2.apply(&mut vg);
        stack.push_current(std::slice::from_ref(&zoom2));

        assert_eq!(
            stack.undo_stack.len(),
            3,
            "NodeAdded between zooms must prevent merge"
        );
    }
}
