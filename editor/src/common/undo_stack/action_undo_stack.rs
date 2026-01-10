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

    #[test]
    fn max_steps_must_be_positive() {
        let result = std::panic::catch_unwind(|| ActionUndoStack::new(0));
        assert!(result.is_err());
    }
}
