use std::fmt::Debug;

mod full_serde_undo_stack;

pub use full_serde_undo_stack::FullSerdeUndoStack;

pub trait UndoStack<T: Debug>: Debug {
    fn reset_with(&mut self, value: &T);
    fn push_current(&mut self, value: &T);
    fn clear_redo(&mut self);
    fn undo(&mut self) -> Option<T>;
    fn redo(&mut self) -> Option<T>;
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
        stack.push_current(&state_b);
        assert!(stack.undo_len() >= 2);

        let undone = stack.undo().expect("undo should return prior state");
        assert_eq!(undone, state_a);
        assert_eq!(stack.redo_len(), 1);

        let redone = stack.redo().expect("redo should return next state");
        assert_eq!(redone, state_b);
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
        stack.push_current(&state_b);
        stack.undo();
        assert_eq!(stack.redo_len(), 1);

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
        stack.push_current(&state_b);
        stack.undo();
        assert_eq!(stack.redo_len(), 1);

        stack.push_current(&state_c);
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
        stack.push_current(&state_b);
        stack.push_current(&state_c);

        assert_eq!(stack.undo_len(), 1);
        assert!(stack.undo().is_none());
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
        stack.push_current(&state_b);
        stack.push_current(&state_c);

        assert_eq!(stack.undo_len(), 2);
        assert_eq!(stack.undo().unwrap(), state_b);
        assert!(stack.undo().is_none());
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
        stack.push_current(&state_b);
        stack.push_current(&state_c);

        assert!(stack.undo_len() <= 1);
    }

    fn max_stack_limit_must_be_positive<F: StackFactory>() {
        let result = std::panic::catch_unwind(|| {
            let _stack = F::make(0);
        });
        assert!(result.is_err(), "stack limit must reject zero");
    }
}
