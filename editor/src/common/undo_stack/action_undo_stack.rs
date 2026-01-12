use std::fmt::Debug;

#[cfg(debug_assertions)]
use common::FileFormat;

#[cfg(debug_assertions)]
use crate::common::undo_stack::FullSerdeUndoStack;
use crate::common::undo_stack::UndoStack;
use crate::gui::graph_ui_interaction::GraphUiAction;
use crate::model::ViewGraph;

#[derive(Debug)]
pub struct ActionUndoStack {
    undo_actions: Vec<u8>,
    redo_actions: Vec<u8>,
    undo_stack: Vec<std::ops::Range<usize>>,
    redo_stack: Vec<std::ops::Range<usize>>,
    undo_base_offset: usize,
    max_steps: usize,
}

impl ActionUndoStack {
    pub fn new(max_steps: usize) -> Self {
        assert!(max_steps > 0, "undo stack must allow at least one step");
        Self {
            undo_actions: Vec::new(),
            redo_actions: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_base_offset: 0,
            max_steps,
        }
    }

    fn trim_to_limit(&mut self) {
        while self.undo_stack.len() > self.max_steps {
            let removed = self.undo_stack.remove(0);
            assert_eq!(
                removed.start, self.undo_base_offset,
                "oldest undo range should start at base offset"
            );
            let offset = self
                .undo_stack
                .first()
                .map(|range| range.start)
                .unwrap_or(self.undo_actions.len());
            assert!(
                offset >= self.undo_base_offset,
                "undo stack offset must not move backwards"
            );
            self.undo_base_offset = offset;
        }

        self.compact_undo_buffer_if_needed();
    }

    fn append_actions(buffer: &mut Vec<u8>, actions: &[GraphUiAction]) -> std::ops::Range<usize> {
        assert!(
            !actions.is_empty(),
            "undo stack should not store empty action batches"
        );
        let start = buffer.len();
        bincode::serde::encode_into_std_write(actions, buffer, bincode::config::standard())
            .expect("undo stack action batch should serialize via bincode");
        let end = buffer.len();
        start..end
    }

    fn deserialize_actions(bytes: &[u8]) -> Vec<GraphUiAction> {
        let (decoded, read) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
            .expect("undo stack action batch should deserialize via bincode");
        assert_eq!(
            read,
            bytes.len(),
            "undo stack action batch should decode fully"
        );
        decoded
    }

    fn append_bytes(target: &mut Vec<u8>, bytes: &[u8]) -> std::ops::Range<usize> {
        let start = target.len();
        target.extend_from_slice(bytes);
        let end = target.len();
        start..end
    }

    fn slice_bytes<'a>(buffer: &'a [u8], range: &std::ops::Range<usize>) -> &'a [u8] {
        assert!(range.start <= range.end, "undo stack range start > end");
        assert!(
            range.end <= buffer.len(),
            "undo stack range exceeds buffer length"
        );
        &buffer[range.clone()]
    }

    fn pop_tail_actions(buffer: &mut Vec<u8>, range: &std::ops::Range<usize>) {
        if range.end == buffer.len() {
            buffer.truncate(range.start);
        }
    }

    fn compact_undo_buffer_if_needed(&mut self) {
        // Skip compaction if there is no logical prefix to drop.
        if self.undo_base_offset == 0 {
            return;
        }
        // Only compact when the unused prefix is at least half the buffer,
        // to avoid frequent memmoves from small trims.
        if self.undo_base_offset < self.undo_actions.len() / 2 {
            return;
        }
        let offset = self.undo_base_offset;
        // Drop the unused prefix and renormalize ranges to the new base.
        self.undo_actions.drain(0..offset);
        for range in &mut self.undo_stack {
            range.start -= offset;
            range.end -= offset;
        }
        self.undo_base_offset = 0;
    }
}

impl UndoStack<ViewGraph> for ActionUndoStack {
    type Action = GraphUiAction;

    fn reset_with(&mut self, _value: &ViewGraph) {
        self.undo_actions.clear();
        self.redo_actions.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_base_offset = 0;
    }

    fn push_current(&mut self, _value: &ViewGraph, actions: &[GraphUiAction]) {
        if actions.is_empty() {
            return;
        }
        self.redo_actions.clear();
        self.redo_stack.clear();
        let range = Self::append_actions(&mut self.undo_actions, actions);
        self.undo_stack.push(range);
        self.trim_to_limit();
    }

    fn clear_redo(&mut self) {
        self.redo_actions.clear();
        self.redo_stack.clear();
    }

    fn undo(&mut self, value: &mut ViewGraph, on_action: &mut dyn FnMut(&GraphUiAction)) -> bool {
        let Some(actions_range) = self.undo_stack.pop() else {
            return false;
        };
        assert!(
            actions_range.start >= self.undo_base_offset,
            "undo range starts before base offset"
        );
        let actions_bytes = {
            let undo_actions = &self.undo_actions;
            Self::slice_bytes(undo_actions, &actions_range)
        };
        let actions = Self::deserialize_actions(actions_bytes);
        for action in actions.iter().rev() {
            action.undo(value);
            on_action(action);
        }
        let redo_range = {
            let redo_actions = &mut self.redo_actions;
            Self::append_bytes(redo_actions, actions_bytes)
        };
        self.redo_stack.push(redo_range);
        Self::pop_tail_actions(&mut self.undo_actions, &actions_range);

        true
    }

    fn redo(&mut self, value: &mut ViewGraph, on_action: &mut dyn FnMut(&GraphUiAction)) -> bool {
        let Some(actions_range) = self.redo_stack.pop() else {
            return false;
        };
        let actions_bytes = {
            let redo_actions = &self.redo_actions;
            Self::slice_bytes(redo_actions, &actions_range)
        };
        let actions = Self::deserialize_actions(actions_bytes);
        for action in actions.iter() {
            action.apply(value);
            on_action(action);
        }
        let undo_range = {
            let undo_actions = &mut self.undo_actions;
            Self::append_bytes(undo_actions, actions_bytes)
        };
        self.undo_stack.push(undo_range);
        Self::pop_tail_actions(&mut self.redo_actions, &actions_range);

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::UiEquals;
    use crate::gui::graph_ui_interaction::EventSubscriberChange;
    use crate::model::view_graph::{IncomingConnection, IncomingEvent};
    use common::FileFormat;
    use egui::{Pos2, Vec2, vec2};
    use graph::data::StaticValue;
    use graph::graph::{Binding, Event, Input, NodeBehavior};
    use graph::prelude::test_graph;

    fn assert_ranges_match_actions(stack: &ActionUndoStack) {
        for range in &stack.undo_stack {
            assert!(range.start <= range.end);
            assert!(range.end <= stack.undo_actions.len());
            assert!(range.start >= stack.undo_base_offset);
        }
        for range in &stack.redo_stack {
            assert!(range.start <= range.end);
            assert!(range.end <= stack.redo_actions.len());
        }

        let undo_actions_len: usize = stack
            .undo_stack
            .iter()
            .map(|range| range.end - range.start)
            .sum();
        let redo_actions_len: usize = stack
            .redo_stack
            .iter()
            .map(|range| range.end - range.start)
            .sum();

        assert_eq!(
            undo_actions_len,
            stack.undo_actions.len() - stack.undo_base_offset
        );
        assert_eq!(redo_actions_len, stack.redo_actions.len());
    }

    fn collect_incoming_connections(
        view_graph: &ViewGraph,
        removed_node_id: graph::graph::NodeId,
    ) -> Vec<IncomingConnection> {
        let mut incoming = Vec::new();
        for node in view_graph.graph.nodes.iter() {
            for (input_idx, input) in node.inputs.iter().enumerate() {
                if let Binding::Bind(address) = &input.binding
                    && address.target_id == removed_node_id
                {
                    incoming.push(IncomingConnection {
                        node_id: node.id,
                        input_idx,
                        binding: input.binding.clone(),
                    });
                }
            }
        }
        incoming
    }

    fn collect_incoming_events(
        view_graph: &ViewGraph,
        removed_node_id: graph::graph::NodeId,
    ) -> Vec<IncomingEvent> {
        let mut incoming = Vec::new();
        for node in view_graph.graph.nodes.iter() {
            for (event_idx, event) in node.events.iter().enumerate() {
                if event.subscribers.contains(&removed_node_id) {
                    incoming.push(IncomingEvent {
                        node_id: node.id,
                        event_idx,
                    });
                }
            }
        }
        incoming
    }

    #[test]
    fn max_steps_must_be_positive() {
        let result = std::panic::catch_unwind(|| ActionUndoStack::new(0));
        assert!(result.is_err());
    }

    #[test]
    fn action_undo_redo_roundtrip() {
        let graph = test_graph();
        let mut view_graph: ViewGraph = graph.into();
        let original = view_graph.serialize(FileFormat::Json);

        let node_id = view_graph.graph.nodes.iter().next().unwrap().id;
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

        let mut stack = ActionUndoStack::new(16);
        stack.reset_with(&view_graph);

        for action in &actions {
            action.apply(&mut view_graph);
        }
        let modified = view_graph.serialize(FileFormat::Json);

        stack.push_current(&view_graph, &actions);

        assert_ranges_match_actions(&stack);
        assert!(stack.undo(&mut view_graph, &mut |_| {}));
        assert_ranges_match_actions(&stack);
        assert_eq!(view_graph.serialize(FileFormat::Json), original);

        assert!(stack.redo(&mut view_graph, &mut |_| {}));
        assert_ranges_match_actions(&stack);
        assert_eq!(view_graph.serialize(FileFormat::Json), modified);
    }

    #[test]
    fn max_steps_limits_history() {
        let graph = test_graph();
        let mut view_graph: ViewGraph = graph.into();
        let node_id = view_graph.graph.nodes.iter().next().unwrap().id;
        let before_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;

        let mut stack = ActionUndoStack::new(1);

        let first_actions = vec![GraphUiAction::NodeMoved {
            node_id,
            before: before_pos,
            after: before_pos + vec2(1.0, 0.0),
        }];
        for action in &first_actions {
            action.apply(&mut view_graph);
        }
        stack.push_current(&view_graph, &first_actions);
        assert_ranges_match_actions(&stack);

        let second_actions = vec![GraphUiAction::NodeMoved {
            node_id,
            before: before_pos + vec2(1.0, 0.0),
            after: before_pos + vec2(2.0, 0.0),
        }];
        for action in &second_actions {
            action.apply(&mut view_graph);
        }
        stack.push_current(&view_graph, &second_actions);
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

    #[test]
    fn undo_base_offset_compacts_when_prefix_large() {
        let graph = test_graph();
        let view_graph: ViewGraph = graph.into();
        let node_id = view_graph.graph.nodes.iter().next().unwrap().id;
        let start_pos = view_graph.view_nodes.by_key(&node_id).unwrap().pos;
        let mut stack = ActionUndoStack::new(2);

        stack.reset_with(&view_graph);

        for i in 0..3 {
            let action = GraphUiAction::NodeMoved {
                node_id,
                before: start_pos + vec2(i as f32, 0.0),
                after: start_pos + vec2((i + 1) as f32, 0.0),
            };
            stack.push_current(&view_graph, &[action]);
        }

        assert!(stack.undo_base_offset > 0);
        assert_ranges_match_actions(&stack);

        let action = GraphUiAction::NodeMoved {
            node_id,
            before: start_pos + vec2(3.0, 0.0),
            after: start_pos + vec2(4.0, 0.0),
        };
        stack.push_current(&view_graph, &[action]);

        assert_eq!(stack.undo_base_offset, 0);
        assert_ranges_match_actions(&stack);
    }

    #[test]
    fn undo_roundtrip_all_action_variants_with_json_snapshots() {
        let graph = test_graph();
        let mut view_graph: ViewGraph = graph.into();
        let mut stack = ActionUndoStack::new(32);

        let node_ids: Vec<_> = view_graph.graph.nodes.iter().map(|node| node.id).collect();
        let primary_id = *node_ids.first().expect("test graph must have nodes");
        let secondary_id = node_ids
            .iter()
            .copied()
            .find(|node_id| *node_id != primary_id)
            .unwrap_or(primary_id);

        if view_graph
            .graph
            .nodes
            .iter()
            .all(|node| node.events.is_empty())
        {
            let node = view_graph
                .graph
                .by_id_mut(&primary_id)
                .expect("primary node must exist");
            node.events.push(Event {
                subscribers: Vec::new(),
            });
        }
        if view_graph
            .graph
            .nodes
            .iter()
            .all(|node| node.inputs.is_empty())
        {
            let node = view_graph
                .graph
                .by_id_mut(&primary_id)
                .expect("primary node must exist");
            node.inputs.push(Input {
                binding: Binding::None,
            });
        }
        let input_node_id = view_graph
            .graph
            .nodes
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

        stack.reset_with(&view_graph);
        let mut snapshots = vec![view_graph.serialize(FileFormat::Json)];

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
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

        let event_node = view_graph
            .graph
            .nodes
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
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

        let before_binding = Binding::None;
        let after_binding = Binding::Const(StaticValue::Int(123));
        let action = GraphUiAction::InputChanged {
            node_id: input_node_id,
            input_idx,
            before: before_binding,
            after: after_binding,
        };
        action.apply(&mut view_graph);
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

        let moved_before = view_graph.view_nodes.by_key(&primary_id).unwrap().pos;
        let moved_after = moved_before + vec2(5.0, -3.0);
        let action = GraphUiAction::NodeMoved {
            node_id: primary_id,
            before: moved_before,
            after: moved_after,
        };
        action.apply(&mut view_graph);
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

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
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

        let zoom_before_pan = view_graph.pan;
        let zoom_before_scale = view_graph.scale;
        let action = GraphUiAction::ZoomPanChanged {
            before_pan: zoom_before_pan,
            before_scale: zoom_before_scale,
            after_pan: zoom_before_pan + vec2(12.0, -6.0),
            after_scale: zoom_before_scale + 0.25,
        };
        action.apply(&mut view_graph);
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

        let mut bound_targets = std::collections::HashSet::new();
        for node in view_graph.graph.nodes.iter() {
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
        let action = view_graph.removal_action(&removed_node_id);
        view_graph.remove_node(&removed_node_id);
        action.apply(&mut view_graph);
        stack.push_current(&view_graph, std::slice::from_ref(&action));
        snapshots.push(view_graph.serialize(FileFormat::Json));

        for (range_idx, range) in stack.undo_stack.iter().enumerate() {
            let bytes = ActionUndoStack::slice_bytes(&stack.undo_actions, range);
            let (decoded, read) = bincode::serde::decode_from_slice::<Vec<GraphUiAction>, _>(
                bytes,
                bincode::config::standard(),
            )
            .unwrap_or_else(|err| panic!("undo range {} failed to decode: {}", range_idx, err));
            assert_eq!(read, bytes.len());
            assert_eq!(decoded.len(), 1);
        }

        for snapshot_idx in (1..snapshots.len()).rev() {
            assert!(stack.undo(&mut view_graph, &mut |_| {}));
            let snapshot_graph =
                ViewGraph::deserialize(FileFormat::Json, &snapshots[snapshot_idx - 1])
                    .expect("snapshot should deserialize");
            assert_eq!(view_graph.graph, snapshot_graph.graph);
            assert_eq!(view_graph.view_nodes, snapshot_graph.view_nodes);
            assert!(view_graph.pan.ui_equals(&snapshot_graph.pan));
            assert!(view_graph.scale.ui_equals(&snapshot_graph.scale));
            assert_eq!(view_graph.selected_node_id, snapshot_graph.selected_node_id);
        }
        assert!(!stack.undo(&mut view_graph, &mut |_| {}));
    }
}
