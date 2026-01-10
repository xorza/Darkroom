use std::fmt::Debug;

mod full_serde_undo_stack;

pub use full_serde_undo_stack::FullSerdeUndoStack;

use crate::gui::graph_ui_interaction::GraphUiAction;

pub trait UndoStack<T: Debug>: Debug {
    fn reset_with(&mut self, value: &T);
    // todo extract trait for GraphUiAction and make it intoiter
    fn push_current(&mut self, value: &T, actions: Vec<GraphUiAction>);
    fn clear_redo(&mut self);
    fn undo(&mut self, value: &mut T) -> bool;
    fn redo(&mut self, value: &mut T) -> bool;
}

#[cfg(test)]
pub mod undo_stack_tests {
    use super::UndoStack;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct TestState {
        pub value: i32,
        pub label: String,
    }

    pub trait UndoStackTestAccess {
        fn undo_len(&self) -> usize;
        fn redo_len(&self) -> usize;
    }

    pub trait StackFactory {
        type Stack: UndoStack<TestState> + UndoStackTestAccess;

        fn make(limit: usize) -> Self::Stack;
        fn limit_for_snapshots(states: &[TestState]) -> usize;
    }

    pub fn run_all<F: StackFactory>() {
        undo_redo_roundtrip::<F>();
        clear_redo_empties_stack::<F>();
        redo_invalidated_on_new_push::<F>();
        undo_stack_drops_oldest_when_over_limit::<F>();
        undo_stack_keeps_two_snapshots_with_two_snapshot_budget::<F>();
        undo_stack_respects_single_snapshot_limit::<F>();
        max_stack_limit_must_be_positive::<F>();
    }

    fn undo_redo_roundtrip<F: StackFactory>() {
        let mut stack = F::make(1024 * 1024);
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        stack.reset_with(&state_a);
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 0);

        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        stack.push_current(&state_b, Vec::new());
        assert!(stack.undo_len() >= 2);

        let mut undone = state_b.clone();
        let did_undo = stack.undo(&mut undone);
        assert_eq!(undone, state_a);
        assert!(did_undo);
        assert_eq!(stack.redo_len(), 1);

        let mut redone = state_a.clone();
        let did_redo = stack.redo(&mut redone);
        assert_eq!(redone, state_b);
        assert!(did_redo);
        assert_eq!(stack.redo_len(), 0);
    }

    fn clear_redo_empties_stack<F: StackFactory>() {
        let mut stack = F::make(1024 * 1024);
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        stack.reset_with(&state_a);
        stack.push_current(&state_b, Vec::new());
        let mut undone = state_b.clone();
        let did_undo = stack.undo(&mut undone);
        assert_eq!(stack.redo_len(), 1);
        assert!(did_undo);

        stack.clear_redo();
        assert_eq!(stack.redo_len(), 0);
    }

    fn redo_invalidated_on_new_push<F: StackFactory>() {
        let mut stack = F::make(1024 * 1024);
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        let state_c = TestState {
            value: 3,
            label: "c".to_string(),
        };
        stack.reset_with(&state_a);
        stack.push_current(&state_b, Vec::new());
        let mut undone = state_b.clone();
        let did_undo = stack.undo(&mut undone);
        assert_eq!(stack.redo_len(), 1);
        assert!(did_undo);

        stack.push_current(&state_c, Vec::new());
        assert_eq!(stack.redo_len(), 0);
    }

    fn undo_stack_drops_oldest_when_over_limit<F: StackFactory>() {
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        let state_c = TestState {
            value: 3,
            label: "c".to_string(),
        };

        let max_limit =
            F::limit_for_snapshots(&[state_a.clone(), state_b.clone(), state_c.clone()]);
        let mut stack = F::make(max_limit);
        stack.reset_with(&state_a);
        stack.push_current(&state_b, Vec::new());
        stack.push_current(&state_c, Vec::new());

        assert_eq!(stack.undo_len(), 1);
        let mut output = state_c.clone();
        let did_undo = stack.undo(&mut output);
        assert_eq!(output, state_c);
        assert!(!did_undo);
    }

    fn undo_stack_keeps_two_snapshots_with_two_snapshot_budget<F: StackFactory>() {
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        let state_c = TestState {
            value: 3,
            label: "c".to_string(),
        };

        let max_limit = F::limit_for_snapshots(&[state_a.clone(), state_b.clone()]);
        let mut stack = F::make(max_limit);
        stack.reset_with(&state_a);
        stack.push_current(&state_b, Vec::new());
        stack.push_current(&state_c, Vec::new());

        assert_eq!(stack.undo_len(), 2);
        let mut output = state_c.clone();
        let did_undo = stack.undo(&mut output);
        assert_eq!(output, state_b);
        assert!(did_undo);
        let did_undo = stack.undo(&mut output);
        assert_eq!(output, state_b);
        assert!(!did_undo);
    }

    fn undo_stack_respects_single_snapshot_limit<F: StackFactory>() {
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        let state_c = TestState {
            value: 3,
            label: "c".to_string(),
        };

        let max_limit =
            F::limit_for_snapshots(&[state_a.clone(), state_b.clone(), state_c.clone()]);
        let mut stack = F::make(max_limit);
        stack.reset_with(&state_a);
        stack.push_current(&state_b, Vec::new());
        stack.push_current(&state_c, Vec::new());

        assert!(stack.undo_len() <= 1);
    }

    fn max_stack_limit_must_be_positive<F: StackFactory>() {
        let result = std::panic::catch_unwind(|| {
            let _stack = F::make(0);
        });
        assert!(result.is_err(), "stack limit must reject zero");
    }
}
