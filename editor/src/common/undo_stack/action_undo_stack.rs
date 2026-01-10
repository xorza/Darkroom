use std::fmt::Debug;

use crate::common::undo_stack::UndoStack;
use crate::gui::graph_ui_interaction::GraphUiAction;
use crate::model::ViewGraph;

#[derive(Debug)]
pub struct ActionUndoStack {
    undo_stack: Vec<Vec<GraphUiAction>>,
    redo_stack: Vec<Vec<GraphUiAction>>,
    max_steps: usize,
}

impl ActionUndoStack {
    pub fn new(max_steps: usize) -> Self {
        assert!(max_steps > 0, "undo stack must allow at least one step");
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_steps,
        }
    }

    fn trim_to_limit(&mut self) {
        while self.undo_stack.len() > self.max_steps {
            self.undo_stack.remove(0);
        }
    }
}

impl UndoStack<ViewGraph> for ActionUndoStack {
    type Action = GraphUiAction;

    fn reset_with(&mut self, _value: &ViewGraph) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    fn push_current(&mut self, _value: &ViewGraph, actions: &[GraphUiAction]) {
        if actions.is_empty() {
            return;
        }
        self.redo_stack.clear();
        self.undo_stack.push(actions.to_vec());
        self.trim_to_limit();
    }

    fn clear_redo(&mut self) {
        self.redo_stack.clear();
    }

    fn undo(&mut self, value: &mut ViewGraph) -> bool {
        let Some(actions) = self.undo_stack.pop() else {
            return false;
        };
        for action in actions.iter().rev() {
            action.undo(value);
        }
        self.redo_stack.push(actions);
        true
    }

    fn redo(&mut self, value: &mut ViewGraph) -> bool {
        let Some(actions) = self.redo_stack.pop() else {
            return false;
        };
        for action in actions.iter() {
            action.apply(value);
        }
        self.undo_stack.push(actions);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::FileFormat;
    use egui::{Pos2, Vec2, vec2};
    use graph::prelude::test_graph;

    #[test]
    fn max_steps_must_be_positive() {
        let result = std::panic::catch_unwind(|| ActionUndoStack::new(0));
        assert!(result.is_err());
    }

    #[test]
    fn action_undo_redo_roundtrip() {
        let graph = test_graph();
        let mut view_graph = ViewGraph::from_graph(&graph);
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

        for action in &actions {
            action.apply(&mut view_graph);
        }
        let modified = view_graph.serialize(FileFormat::Json);

        let mut stack = ActionUndoStack::new(16);
        stack.push_current(&view_graph, &actions);

        assert!(stack.undo(&mut view_graph));
        assert_eq!(view_graph.serialize(FileFormat::Json), original);

        assert!(stack.redo(&mut view_graph));
        assert_eq!(view_graph.serialize(FileFormat::Json), modified);
    }

    #[test]
    fn max_steps_limits_history() {
        let graph = test_graph();
        let mut view_graph = ViewGraph::from_graph(&graph);
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

        let second_actions = vec![GraphUiAction::NodeMoved {
            node_id,
            before: before_pos + vec2(1.0, 0.0),
            after: before_pos + vec2(2.0, 0.0),
        }];
        for action in &second_actions {
            action.apply(&mut view_graph);
        }
        stack.push_current(&view_graph, &second_actions);

        assert!(stack.undo(&mut view_graph));
        assert_eq!(
            view_graph.view_nodes.by_key(&node_id).unwrap().pos,
            before_pos + vec2(1.0, 0.0)
        );
        assert!(!stack.undo(&mut view_graph));
    }
}
