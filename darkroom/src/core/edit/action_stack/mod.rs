//! Bitcode-packed undo/redo history in one byte buffer.
//!
//! Every entry — undoable *and* redoable — lives packed back-to-back in a
//! single `actions: Vec<u8>`, with a parallel `entries` table of
//! `(range, gesture_key, target)`. A `cursor` splits the applied entries
//! (`entries[..cursor]`, undoable) from the undone ones
//! (`entries[cursor..]`, redoable). Undo/redo are just a cursor step plus
//! one deserialize — no second buffer, no copying bytes between buffers.
//! A fresh edit discards the redoable tail (truncate from the end),
//! appends, and trims the oldest entries off the front to honor a byte
//! budget.
//!
//! Front eviction is O(1): a `head` marks the first live byte, so
//! dropping the oldest entry just advances `head` (and pops the front of
//! the `VecDeque` metadata) — no memmove. The dead `[0, head)` prefix is
//! reclaimed lazily by a `drain` once it grows past the budget, so that
//! one memmove amortizes over a whole budget of evictions. The history is
//! still one contiguous `actions` allocation plus a single ring buffer of
//! fixed-size metadata — no per-entry / per-field churn (the naive
//! `VecDeque<Vec<UndoStep>>` form re-allocated a `RemoveNode`'s `Node` +
//! captured wiring on every entry).

use std::collections::VecDeque;
use std::ops::Range;

use common::SerdeFormat;

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::apply::{apply_step, revert_step};
use crate::core::edit::intent::types::{GestureKey, UndoStep};

#[derive(Debug)]
struct Entry {
    /// Byte range of this entry's serialized steps in `actions`.
    range: Range<usize>,
    /// Cached so gesture-merge can reject without deserializing the
    /// entry. Only set for single-step batches that identify as a
    /// gesture.
    gesture_key: Option<GestureKey>,
    /// Which graph this batch mutated, so undo/redo re-target the right
    /// graph+view even when the user has since switched tabs.
    target: GraphRef,
}

#[derive(Debug)]
pub(crate) struct ActionStack {
    /// The single packed history buffer. Live entries occupy
    /// `[head, len)`; `[0, head)` is evicted-but-not-yet-reclaimed dead
    /// prefix. Entry ranges are absolute indices into this buffer.
    actions: Vec<u8>,
    /// First live byte of `actions`. Advanced on eviction (O(1)),
    /// reclaimed by `trim_to_limit`'s lazy compaction.
    head: usize,
    /// Per-entry metadata, parallel to the packed entries in `actions`.
    /// A `VecDeque` so front eviction (`pop_front`) is O(1) too.
    entries: VecDeque<Entry>,
    /// Boundary between applied (`entries[..cursor]`) and undone
    /// (`entries[cursor..]`) entries. Undo decrements, redo increments, a
    /// new edit truncates the undone tail then appends.
    cursor: usize,
    /// Live-byte budget (`len - head`). When a push overflows it the
    /// oldest entries are dropped off the front; the just-pushed entry
    /// always survives, even when it alone exceeds the budget. Bounds
    /// history by memory rather than entry count, since one entry (e.g. a
    /// `RemoveNode` carrying a whole `Node` + wiring) can dwarf many small
    /// ones. Physical `actions` peaks at ~2× this between compactions.
    max_bytes: usize,
}

impl ActionStack {
    pub(crate) fn new(max_bytes: usize) -> Self {
        assert!(max_bytes > 0, "undo history needs a positive byte budget");
        Self {
            actions: Vec::new(),
            head: 0,
            entries: VecDeque::new(),
            cursor: 0,
            max_bytes,
        }
    }

    /// Push a batch of just-applied steps that mutated `target`. `steps`
    /// is a single undo entry — undoing/redoing replays the whole batch
    /// atomically against `target`'s graph+view.
    pub(crate) fn push_current(&mut self, target: GraphRef, steps: &[UndoStep]) {
        if steps.is_empty() {
            return;
        }
        // A fresh edit makes the undone tail unreachable; drop it.
        self.discard_redo();

        // A gesture key only exists for single-step batches; a multi-step
        // batch (e.g. a breaker swipe) is never coalesced.
        let gesture_key = match steps {
            [step] => step.gesture_key(),
            _ => None,
        };

        // Gesture coalescing: if this push matches the previous entry's
        // cached key *on the same graph*, merge in place — keep the
        // existing "from" half, replace the "to" half. Cross-frame
        // zoom/pan collapses to one undo step.
        if let Some(key) = gesture_key
            && self.try_merge_with_last(&steps[0], key, target)
        {
            return;
        }

        let range = Self::append_steps(&mut self.actions, steps);
        self.entries.push_back(Entry {
            range,
            gesture_key,
            target,
        });
        self.cursor = self.entries.len();
        self.trim_to_limit();
    }

    pub(crate) fn undo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        // `revert_step` resolves the right graph+view from `target` and
        // no-ops if it's gone (graph deleted). The entry stays in the
        // buffer — it just moved into the redoable region.
        let entry = &self.entries[self.cursor];
        let target = entry.target;
        let steps = Self::deserialize_steps(Self::slice_bytes(&self.actions, &entry.range));
        for step in steps.iter().rev() {
            revert_step(step, doc, target);
            on_step(step);
        }
        true
    }

    pub(crate) fn redo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        if self.cursor == self.entries.len() {
            return false;
        }
        let entry = &self.entries[self.cursor];
        let target = entry.target;
        let steps = Self::deserialize_steps(Self::slice_bytes(&self.actions, &entry.range));
        for step in steps.iter() {
            apply_step(step, doc, target);
            on_step(step);
        }
        self.cursor += 1;
        true
    }

    /// Drop the redoable tail (`entries[cursor..]`) and its bytes — a new
    /// edit makes them unreachable. Truncations from the end, so no
    /// memmove. If that empties the live region (a full rewind then a
    /// fresh edit), reclaim the dead prefix too.
    fn discard_redo(&mut self) {
        if self.cursor < self.entries.len() {
            let cut = self.entries[self.cursor].range.start;
            self.actions.truncate(cut);
            self.entries.truncate(self.cursor);
            if self.entries.is_empty() {
                self.actions.clear();
                self.head = 0;
            }
        }
    }

    fn trim_to_limit(&mut self) {
        // Runs only right after a push, where every entry is applied — the
        // eviction `cursor -= 1` below relies on it.
        debug_assert_eq!(
            self.cursor,
            self.entries.len(),
            "trim_to_limit expects all entries applied"
        );
        // Drop the oldest entries until the live region fits the budget —
        // minimal history loss. Each drop just advances `head` past the
        // freed bytes and pops the front metadata entry: O(1), no memmove.
        // Always keep the last (just-pushed) entry.
        while self.entries.len() > 1 && self.actions.len() - self.head > self.max_bytes {
            let removed = self.entries.pop_front().unwrap();
            self.head = removed.range.end;
            self.cursor -= 1;
        }
        // Reclaim the dead prefix lazily, once it has grown past the
        // budget — the one memmove amortizes over a budget of evictions,
        // and physical `actions` stays ~2× the budget.
        if self.head > self.max_bytes {
            self.actions.drain(0..self.head);
            for entry in &mut self.entries {
                entry.range.start -= self.head;
                entry.range.end -= self.head;
            }
            self.head = 0;
        }
        // `head` always marks the oldest live entry's start (0 when empty)
        // — the dead prefix ends exactly where live history begins.
        debug_assert!(
            self.entries
                .front()
                .map_or(self.head == 0, |e| e.range.start == self.head),
            "head must mark the oldest live entry's start"
        );
    }

    fn try_merge_with_last(
        &mut self,
        new_step: &UndoStep,
        key: GestureKey,
        target: GraphRef,
    ) -> bool {
        // `discard_redo` ran first, so the last entry is the last applied
        // one and its bytes are the buffer tail.
        let Some(last) = self.entries.back() else {
            return false;
        };
        if last.gesture_key != Some(key) || last.target != target {
            return false;
        }
        let last_range = last.range.clone();
        let last_bytes = Self::slice_bytes(&self.actions, &last_range);
        let last_steps = Self::deserialize_steps(last_bytes);
        assert_eq!(
            last_steps.len(),
            1,
            "gesture-keyed entry must hold a single step"
        );

        // Fold the existing entry's "from" half with the incoming step's
        // "to" half. `intent` owns the per-variant logic; the
        // `gesture_key == Some(key)` gate above already guarantees a
        // matching variant (and same grabbed member, for `SelectionDrag`).
        let Some(merged) = last_steps[0].coalesce(new_step) else {
            return false;
        };

        // The last entry is the buffer tail — truncate it off and
        // re-append the merged step in place.
        self.actions.truncate(last_range.start);
        self.entries.pop_back();
        let range = Self::append_steps(&mut self.actions, &[merged]);
        self.entries.push_back(Entry {
            range,
            gesture_key: Some(key),
            target,
        });
        self.cursor = self.entries.len();
        true
    }

    fn append_steps(buffer: &mut Vec<u8>, steps: &[UndoStep]) -> Range<usize> {
        assert!(
            !steps.is_empty(),
            "undo stack should not store empty step batches"
        );
        let start = buffer.len();
        common::serde::serialize_into(steps, SerdeFormat::Bitcode, buffer, &mut Vec::new())
            .expect("bitcode serialize of in-memory undo steps is infallible");
        let end = buffer.len();
        start..end
    }

    fn deserialize_steps(bytes: &[u8]) -> Vec<UndoStep> {
        common::serde::deserialize(bytes, SerdeFormat::Bitcode).unwrap()
    }

    fn slice_bytes<'a>(buffer: &'a [u8], range: &Range<usize>) -> &'a [u8] {
        assert!(range.start <= range.end, "undo stack range start > end");
        assert!(
            range.end <= buffer.len(),
            "undo stack range exceeds buffer length"
        );
        &buffer[range.clone()]
    }
}

#[cfg(test)]
mod tests;
