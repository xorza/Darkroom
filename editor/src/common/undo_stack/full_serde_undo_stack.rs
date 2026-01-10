use std::fmt::Debug;

use common::FileFormat;
use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::common::undo_stack::UndoStack;

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
        self.push_current(value);
    }

    pub fn push_current(&mut self, value: &T) {
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

    pub fn undo(&mut self) -> Option<T> {
        if self.undo_stack.len() < 2 {
            return None;
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
        Some(deserialize_snapshot(
            slice_from_range(&self.undo_bytes, snapshot),
            self.format,
        ))
    }

    pub fn redo(&mut self) -> Option<T> {
        let snapshot = self.redo_stack.pop()?;
        let snapshot_bytes = slice_from_range(&self.redo_bytes, &snapshot).to_vec();
        let undo_range = append_bytes(&mut self.undo_bytes, &snapshot_bytes);
        self.undo_stack.push(undo_range);
        enforce_stack_limit(
            &mut self.undo_stack,
            &mut self.undo_bytes,
            self.max_stack_bytes,
        );
        pop_tail_bytes(&mut self.redo_bytes, &snapshot);
        Some(deserialize_snapshot(&snapshot_bytes, self.format))
    }
}

impl<T: Debug> UndoStack<T> for FullSerdeUndoStack<T>
where
    T: Serialize + DeserializeOwned,
{
    fn reset_with(&mut self, value: &T) {
        FullSerdeUndoStack::reset_with(self, value);
    }

    fn push_current(&mut self, value: &T) {
        FullSerdeUndoStack::push_current(self, value);
    }

    fn clear_redo(&mut self) {
        FullSerdeUndoStack::clear_redo(self);
    }

    fn undo(&mut self) -> Option<T> {
        FullSerdeUndoStack::undo(self)
    }

    fn redo(&mut self) -> Option<T> {
        FullSerdeUndoStack::redo(self)
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
    use crate::common::undo_stack::undo_stack_tests::{
        StackFactory, TestState, UndoStackTestAccess, run_all,
    };

    struct FullSerdeFactory;

    impl UndoStackTestAccess for FullSerdeUndoStack<TestState> {
        fn undo_len(&self) -> usize {
            self.undo_stack.len()
        }

        fn redo_len(&self) -> usize {
            self.redo_stack.len()
        }
    }

    impl StackFactory for FullSerdeFactory {
        type Stack = FullSerdeUndoStack<TestState>;

        fn make(limit: usize) -> Self::Stack {
            FullSerdeUndoStack::new(FileFormat::Json, limit)
        }

        fn limit_for_snapshots(states: &[TestState]) -> usize {
            let mut max_len = 0;
            let mut sum_len = 0;
            for state in states {
                let snapshot = serialize_snapshot(state, FileFormat::Json);
                let len = snapshot.len();
                max_len = max_len.max(len);
                sum_len += len;
            }
            match states.len() {
                0 => 1,
                1 => max_len.max(1),
                2 => sum_len.max(1),
                _ => max_len.max(1),
            }
        }
    }

    #[test]
    fn full_serde_undo_stack_suite() {
        run_all::<FullSerdeFactory>();
    }
}
