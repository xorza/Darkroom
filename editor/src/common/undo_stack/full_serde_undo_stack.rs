use std::fmt::Debug;

use common::FileFormat;
use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::common::undo_stack::UndoStack;
use crate::gui::graph_ui_interaction::GraphUiAction;

#[derive(Debug)]
pub struct FullSerdeUndoStack<T: Debug> {
    undo_bytes: Vec<u8>,
    redo_bytes: Vec<u8>,
    undo_stack: Vec<std::ops::Range<usize>>,
    redo_stack: Vec<std::ops::Range<usize>>,
    format: FileFormat,
    max_stack_bytes: usize,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Debug> FullSerdeUndoStack<T>
where
    T: Serialize + DeserializeOwned,
{
    pub fn new(format: FileFormat, max_stack_bytes: usize) -> Self {
        assert!(
            max_stack_bytes > 0,
            "undo stack byte limit must be greater than 0"
        );
        Self {
            undo_bytes: Vec::new(),
            redo_bytes: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            format,
            max_stack_bytes,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn reset_with(&mut self, value: &T) {
        self.undo_bytes.clear();
        self.redo_bytes.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.push_current(value, &[]);
    }

    pub fn push_current(&mut self, value: &T, actions: &[GraphUiAction]) {
        let _ = actions;
        let snapshot = serialize_snapshot(value, self.format);
        if self
            .undo_stack
            .last()
            .is_some_and(|last| snapshot_matches(&self.undo_bytes, last, &snapshot))
        {
            return;
        }
        self.clear_redo();
        let range = append_bytes(&mut self.undo_bytes, &snapshot);
        self.undo_stack.push(range);
        enforce_stack_limit(
            &mut self.undo_stack,
            &mut self.undo_bytes,
            self.max_stack_bytes,
        );
    }

    pub fn clear_redo(&mut self) {
        self.redo_bytes.clear();
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, value: &mut T) -> bool {
        if self.undo_stack.len() < 2 {
            return false;
        }

        let current = self
            .undo_stack
            .pop()
            .expect("undo stack should contain current snapshot");
        let current_bytes = slice_from_range(&self.undo_bytes, &current).to_vec();
        let redo_range = append_bytes(&mut self.redo_bytes, &current_bytes);
        self.redo_stack.push(redo_range);
        enforce_stack_limit(
            &mut self.redo_stack,
            &mut self.redo_bytes,
            self.max_stack_bytes,
        );
        pop_tail_bytes(&mut self.undo_bytes, &current);

        let snapshot = self
            .undo_stack
            .last()
            .expect("undo stack should contain a prior snapshot");
        *value = deserialize_snapshot(slice_from_range(&self.undo_bytes, snapshot), self.format);
        true
    }

    pub fn redo(&mut self, value: &mut T) -> bool {
        let Some(snapshot) = self.redo_stack.pop() else {
            return false;
        };
        let snapshot_bytes = slice_from_range(&self.redo_bytes, &snapshot).to_vec();
        let undo_range = append_bytes(&mut self.undo_bytes, &snapshot_bytes);
        self.undo_stack.push(undo_range);
        enforce_stack_limit(
            &mut self.undo_stack,
            &mut self.undo_bytes,
            self.max_stack_bytes,
        );
        pop_tail_bytes(&mut self.redo_bytes, &snapshot);
        *value = deserialize_snapshot(&snapshot_bytes, self.format);
        true
    }
}

impl<T: Debug> UndoStack<T> for FullSerdeUndoStack<T>
where
    T: Serialize + DeserializeOwned,
{
    type Action = GraphUiAction;

    fn reset_with(&mut self, value: &T) {
        FullSerdeUndoStack::reset_with(self, value);
    }

    fn push_current(&mut self, value: &T, actions: &[GraphUiAction]) {
        FullSerdeUndoStack::push_current(self, value, actions);
    }

    fn clear_redo(&mut self) {
        FullSerdeUndoStack::clear_redo(self);
    }

    fn undo(&mut self, value: &mut T) -> bool {
        FullSerdeUndoStack::undo(self, value)
    }

    fn redo(&mut self, value: &mut T) -> bool {
        FullSerdeUndoStack::redo(self, value)
    }
}

fn serialize_snapshot<T: Serialize>(value: &T, format: FileFormat) -> Vec<u8> {
    let serialized = common::serialize(value, format);
    compress_prepend_size(serialized.as_bytes())
}

fn deserialize_snapshot<T: DeserializeOwned>(snapshot: &[u8], format: FileFormat) -> T {
    let decompressed =
        decompress_size_prepended(snapshot).expect("undo snapshot should decompress");
    let decoded = String::from_utf8(decompressed).expect("undo snapshot should be valid UTF-8");
    common::deserialize(&decoded, format).expect("undo snapshot should deserialize into a value")
}

fn append_bytes(target: &mut Vec<u8>, bytes: &[u8]) -> std::ops::Range<usize> {
    let start = target.len();
    target.extend_from_slice(bytes);
    let end = target.len();
    start..end
}

fn slice_from_range<'a>(bytes: &'a [u8], range: &std::ops::Range<usize>) -> &'a [u8] {
    &bytes[range.clone()]
}

fn snapshot_matches(bytes: &[u8], range: &std::ops::Range<usize>, snapshot: &[u8]) -> bool {
    slice_from_range(bytes, range) == snapshot
}

fn enforce_stack_limit(
    stack: &mut Vec<std::ops::Range<usize>>,
    bytes: &mut Vec<u8>,
    max_stack_bytes: usize,
) {
    assert!(
        max_stack_bytes > 0,
        "undo stack byte limit must be greater than 0"
    );
    while !bytes.is_empty() && bytes.len() > max_stack_bytes {
        let removed = stack.remove(0);
        assert_eq!(removed.start, 0, "oldest snapshot range should start at 0");
        let removed_len = removed.end - removed.start;
        assert!(
            removed_len <= bytes.len(),
            "undo snapshot range exceeds buffer"
        );
        bytes.drain(0..removed.end);
        for range in stack.iter_mut() {
            range.start -= removed_len;
            range.end -= removed_len;
        }
    }
}

fn pop_tail_bytes(bytes: &mut Vec<u8>, range: &std::ops::Range<usize>) {
    if range.end == bytes.len() {
        bytes.truncate(range.start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestState {
        value: i32,
        label: String,
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut stack = FullSerdeUndoStack::new(FileFormat::Json, 1024 * 1024);
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        stack.reset_with(&state_a);
        assert_eq!(stack.undo_stack.len(), 1);
        assert_eq!(stack.redo_stack.len(), 0);

        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        stack.push_current(&state_b, &[]);
        assert!(stack.undo_stack.len() >= 2);

        let mut undone = state_b.clone();
        let did_undo = stack.undo(&mut undone);
        assert_eq!(undone, state_a);
        assert!(did_undo);
        assert_eq!(stack.redo_stack.len(), 1);

        let mut redone = state_a.clone();
        let did_redo = stack.redo(&mut redone);
        assert_eq!(redone, state_b);
        assert!(did_redo);
        assert_eq!(stack.redo_stack.len(), 0);
    }

    #[test]
    fn clear_redo_empties_stack() {
        let mut stack = FullSerdeUndoStack::new(FileFormat::Json, 1024 * 1024);
        let state_a = TestState {
            value: 1,
            label: "a".to_string(),
        };
        let state_b = TestState {
            value: 2,
            label: "b".to_string(),
        };
        stack.reset_with(&state_a);
        stack.push_current(&state_b, &[]);
        let mut undone = state_b.clone();
        let did_undo = stack.undo(&mut undone);
        assert!(did_undo);
        assert_eq!(stack.redo_stack.len(), 1);

        stack.clear_redo();
        assert_eq!(stack.redo_stack.len(), 0);
    }

    #[test]
    fn redo_invalidated_on_new_push() {
        let mut stack = FullSerdeUndoStack::new(FileFormat::Json, 1024 * 1024);
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
        stack.push_current(&state_b, &[]);
        let mut undone = state_b.clone();
        let did_undo = stack.undo(&mut undone);
        assert!(did_undo);
        assert_eq!(stack.redo_stack.len(), 1);

        stack.push_current(&state_c, &[]);
        assert_eq!(stack.redo_stack.len(), 0);
    }

    #[test]
    fn undo_stack_drops_oldest_when_over_limit() {
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

        let snapshot_a = serialize_snapshot(&state_a, FileFormat::Json);
        let snapshot_b = serialize_snapshot(&state_b, FileFormat::Json);
        let snapshot_c = serialize_snapshot(&state_c, FileFormat::Json);
        let max_limit = snapshot_a
            .len()
            .max(snapshot_b.len())
            .max(snapshot_c.len())
            .max(1);
        let mut stack = FullSerdeUndoStack::new(FileFormat::Json, max_limit);
        stack.reset_with(&state_a);
        stack.push_current(&state_b, &[]);
        stack.push_current(&state_c, &[]);

        assert_eq!(stack.undo_stack.len(), 1);
        let mut output = state_c.clone();
        let did_undo = stack.undo(&mut output);
        assert_eq!(output, state_c);
        assert!(!did_undo);
    }

    #[test]
    fn undo_stack_keeps_two_snapshots_with_two_snapshot_budget() {
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

        let snapshot_a = serialize_snapshot(&state_a, FileFormat::Json);
        let snapshot_b = serialize_snapshot(&state_b, FileFormat::Json);
        let max_limit = (snapshot_a.len() + snapshot_b.len()).max(1);
        let mut stack = FullSerdeUndoStack::new(FileFormat::Json, max_limit);
        stack.reset_with(&state_a);
        stack.push_current(&state_b, &[]);
        stack.push_current(&state_c, &[]);

        assert_eq!(stack.undo_stack.len(), 2);
        let mut output = state_c.clone();
        let did_undo = stack.undo(&mut output);
        assert_eq!(output, state_b);
        assert!(did_undo);
        let did_undo = stack.undo(&mut output);
        assert_eq!(output, state_b);
        assert!(!did_undo);
    }

    #[test]
    fn undo_stack_respects_single_snapshot_limit() {
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

        let snapshot_a = serialize_snapshot(&state_a, FileFormat::Json);
        let snapshot_b = serialize_snapshot(&state_b, FileFormat::Json);
        let snapshot_c = serialize_snapshot(&state_c, FileFormat::Json);
        let max_limit = snapshot_a
            .len()
            .max(snapshot_b.len())
            .max(snapshot_c.len())
            .max(1);
        let mut stack = FullSerdeUndoStack::new(FileFormat::Json, max_limit);
        stack.reset_with(&state_a);
        stack.push_current(&state_b, &[]);
        stack.push_current(&state_c, &[]);

        assert!(stack.undo_stack.len() <= 1);
    }

    #[test]
    #[should_panic(expected = "undo stack byte limit must be greater than 0")]
    fn max_stack_bytes_must_be_positive() {
        let _stack: FullSerdeUndoStack<TestState> = FullSerdeUndoStack::new(FileFormat::Json, 0);
    }
}
