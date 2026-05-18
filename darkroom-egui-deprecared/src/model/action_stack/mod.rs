use std::fmt::Debug;
use std::ops::Range;

use common::SerdeFormat;

use crate::model::ViewGraph;
use crate::model::intent::{self, GestureKey, UndoStep};

#[cfg(test)]
mod tests;

#[derive(Debug)]
struct UndoEntry {
    range: Range<usize>,
    // Cached so gesture-merge can reject without deserializing the entry.
    // Only set for single-step batches that identify as a gesture.
    gesture_key: Option<GestureKey>,
}

/// Undo history kept as two flat byte buffers with per-entry ranges,
/// rather than `VecDeque<Vec<UndoStep>>`.
///
/// Why: an undo entry for `RemoveNode` carries a full `Node` + per-edge
/// `Vec<IncomingConnection>` / `Vec<IncomingEvent>` inside its
/// `Snapshot`; a naive enum-storage history hits the allocator once per
/// field per pushed batch and leaves the heap fragmented as the ring
/// trims old entries. Bitcode-packing into one contiguous buffer keeps
/// the whole history in two allocations (undo + redo) regardless of
/// entry count, makes `trim_to_limit` a single `Vec::drain` of the
/// packed prefix, and keeps cache locality when we walk the stack.
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

    /// Push a batch of just-applied steps. `steps` is a single undo
    /// entry — undoing/redoing replays the whole batch atomically.
    pub fn push_current(&mut self, steps: &[UndoStep]) {
        if steps.is_empty() {
            return;
        }
        self.redo_actions.clear();
        self.redo_stack.clear();

        // Gesture coalescing: if this push is a single step matching the
        // previous entry's cached gesture_key, merge in place — keep the
        // existing "from" half, replace the "to" half. Cross-frame
        // zoom/pan collapses to one undo step without cross-frame state.
        if steps.len() == 1
            && let Some(key) = intent::gesture_key(&steps[0])
            && self.try_merge_with_last(&steps[0], key)
        {
            return;
        }

        let range = Self::append_steps(&mut self.undo_actions, steps, &mut self.temp_buffer);
        let gesture_key = if steps.len() == 1 {
            intent::gesture_key(&steps[0])
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

    pub fn undo(&mut self, view_graph: &mut ViewGraph, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        let bytes = Self::slice_bytes(&self.undo_actions, &entry.range);
        let steps = Self::deserialize_steps(bytes, &mut self.temp_buffer);
        for step in steps.iter().rev() {
            intent::revert_step(step, view_graph);
            on_step(step);
        }
        let redo_range = Self::append_bytes(&mut self.redo_actions, bytes);
        self.redo_stack.push(redo_range);
        Self::pop_tail_actions(&mut self.undo_actions, &entry.range);

        true
    }

    pub fn redo(&mut self, view_graph: &mut ViewGraph, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        let Some(range) = self.redo_stack.pop() else {
            return false;
        };
        let bytes = Self::slice_bytes(&self.redo_actions, &range);
        let steps = Self::deserialize_steps(bytes, &mut self.temp_buffer);
        let gesture_key = if steps.len() == 1 {
            intent::gesture_key(&steps[0])
        } else {
            None
        };
        for step in steps.iter() {
            intent::apply_step(step, view_graph);
            on_step(step);
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

    /// If the previous undo entry has the same gesture key as
    /// `new_step`, replace its intent with `new_step.intent` while
    /// keeping its existing snapshot. The captured snapshot from the
    /// gesture's first frame is exactly what undo wants — discarding
    /// later snapshots would lose the gesture's true starting state.
    fn try_merge_with_last(&mut self, new_step: &UndoStep, key: GestureKey) -> bool {
        let Some(last) = self.undo_stack.last() else {
            return false;
        };
        if last.gesture_key != Some(key) {
            return false;
        }
        let last_range = last.range.clone();
        let last_bytes = Self::slice_bytes(&self.undo_actions, &last_range);
        let last_steps = Self::deserialize_steps(last_bytes, &mut self.temp_buffer);
        assert_eq!(
            last_steps.len(),
            1,
            "gesture-keyed entry must hold a single step"
        );

        // Combine the "from" half of the existing entry with the "to"
        // half of the incoming step. Per-variant because each variant's
        // forward/backward field shape differs. Add a match arm here
        // when introducing a new gesture variant in `gesture_key`.
        //
        // The `gesture_key == Some(key)` check above guarantees the
        // pair is the same variant *and*, for `NodeDrag`, the same
        // node id — so MoveNode→MoveNode here is always a same-node
        // continuous drag.
        let merged = match (&last_steps[0], new_step) {
            (
                UndoStep::SetViewport {
                    from_pan,
                    from_scale,
                    ..
                },
                UndoStep::SetViewport {
                    to_pan, to_scale, ..
                },
            ) => UndoStep::SetViewport {
                from_pan: *from_pan,
                from_scale: *from_scale,
                to_pan: *to_pan,
                to_scale: *to_scale,
            },
            (UndoStep::MoveNode { node_id, from, .. }, UndoStep::MoveNode { to, .. }) => {
                UndoStep::MoveNode {
                    node_id: *node_id,
                    from: *from,
                    to: *to,
                }
            }
            _ => return false,
        };

        Self::pop_tail_actions(&mut self.undo_actions, &last_range);
        self.undo_stack.pop();
        let range = Self::append_steps(&mut self.undo_actions, &[merged], &mut self.temp_buffer);
        self.undo_stack.push(UndoEntry {
            range,
            gesture_key: Some(key),
        });
        true
    }

    fn append_steps(
        buffer: &mut Vec<u8>,
        steps: &[UndoStep],
        temp_buffer: &mut Vec<u8>,
    ) -> Range<usize> {
        assert!(
            !steps.is_empty(),
            "undo stack should not store empty step batches"
        );
        let start = buffer.len();
        common::serde::serialize_into(steps, SerdeFormat::Bitcode, buffer, temp_buffer);
        let end = buffer.len();
        start..end
    }

    fn deserialize_steps(bytes: &[u8], temp_buffer: &mut Vec<u8>) -> Vec<UndoStep> {
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
