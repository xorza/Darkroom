//! Bitcode-packed undo/redo history in one byte buffer.
//!
//! Every entry — undoable *and* redoable — lives packed back-to-back in a
//! single `actions: Vec<u8>`, with a parallel `entries` table of byte ranges.
//! A `cursor` splits the applied entries (`entries[..cursor]`, undoable) from
//! the undone ones (`entries[cursor..]`, redoable). Undo/redo are just a
//! cursor step plus one deserialize — no second buffer, no copying bytes
//! between buffers. A fresh edit discards the redoable tail (truncate from
//! the end), appends, and trims the oldest entries off the front to honor a
//! byte budget.
//!
//! Front eviction is O(1): a `head` marks the first live byte, so dropping
//! the oldest entry just advances `head` (and pops the front of the
//! `VecDeque` of ranges) — no memmove. The dead `[0, head)` prefix is
//! reclaimed lazily by a `drain` once it grows past the budget, so that one
//! memmove amortizes over a whole budget of evictions.
//!
//! A held gesture is the one entry not packed yet. Its frames fold into one
//! decoded step in place, and the step is packed once, when the gesture is
//! sealed: by the next edit, an undo or a redo. A drag frame therefore
//! encodes nothing, and a gesture that ends where it started records nothing.

use std::collections::VecDeque;
use std::ops::Range;
use std::slice;

use common::SerdeFormat;

use crate::core::document::Document;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::step::undo_step::UndoStep;

/// The newest entry, while its gesture is still held: decoded, so each frame
/// folds into it in place.
#[derive(Debug)]
struct OpenGesture {
    id: GestureId,
    step: UndoStep,
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
    /// Byte range of each packed entry's serialized steps in `actions`.
    /// A `VecDeque` so front eviction (`pop_front`) is O(1) too.
    entries: VecDeque<Range<usize>>,
    /// Boundary between applied (`entries[..cursor]`) and undone
    /// (`entries[cursor..]`) entries. Undo decrements, redo increments, a
    /// new edit truncates the undone tail then appends.
    cursor: usize,
    /// The held gesture's entry, applied and newer than every packed one.
    /// While it is open there is no undone tail: opening it discarded that.
    open: Option<OpenGesture>,
    /// Live-byte budget (`len - head`). When a push overflows it the
    /// oldest entries are dropped off the front; the just-pushed entry
    /// always survives, even when it alone exceeds the budget. Bounds
    /// history by memory rather than entry count, since one entry (e.g. a
    /// removal carrying a whole `Node` + wiring) can dwarf many small
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
            open: None,
            max_bytes,
        }
    }

    /// Record a batch of just-applied steps as one undo entry — undoing or
    /// redoing replays the whole batch — and leave `steps` empty, its
    /// capacity kept for the next frame.
    ///
    /// `gesture` names the held gesture a one-step batch is a frame of. A
    /// frame of the open gesture folds into its entry; any other batch seals
    /// that entry first.
    pub(crate) fn push(&mut self, steps: &mut Vec<UndoStep>, gesture: Option<GestureId>) {
        if steps.is_empty() {
            return;
        }
        if let Some(id) = gesture {
            debug_assert_eq!(steps.len(), 1, "a gesture frame is one step");
            let step = steps.pop().unwrap();
            if let Some(open) = &mut self.open
                && open.id == id
            {
                open.step.absorb(&step);
                return;
            }
            self.seal();
            // A fresh edit makes the undone tail unreachable; drop it.
            self.discard_redo();
            self.open = Some(OpenGesture { id, step });
            return;
        }
        self.seal();
        self.discard_redo();
        self.pack(steps);
        steps.clear();
    }

    pub(crate) fn undo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        self.seal();
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        // The entry stays in the buffer — it just moved into the redoable
        // region.
        let range = &self.entries[self.cursor];
        let steps = Self::deserialize_steps(Self::slice_bytes(&self.actions, range));
        for step in steps.iter().rev() {
            step.revert(doc);
            on_step(step);
        }
        true
    }

    pub(crate) fn redo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        self.seal();
        if self.cursor == self.entries.len() {
            return false;
        }
        let range = &self.entries[self.cursor];
        let steps = Self::deserialize_steps(Self::slice_bytes(&self.actions, range));
        for step in &steps {
            step.apply(doc);
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
            let cut = self.entries[self.cursor].start;
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
            self.head = removed.end;
            self.cursor -= 1;
        }
        // Reclaim the dead prefix lazily, once it has grown past the
        // budget — the one memmove amortizes over a budget of evictions,
        // and physical `actions` stays ~2× the budget.
        if self.head > self.max_bytes {
            self.actions.drain(0..self.head);
            for range in &mut self.entries {
                range.start -= self.head;
                range.end -= self.head;
            }
            self.head = 0;
        }
        // `head` always marks the oldest live entry's start (0 when empty)
        // — the dead prefix ends exactly where live history begins.
        debug_assert!(
            self.entries
                .front()
                .map_or(self.head == 0, |range| range.start == self.head),
            "head must mark the oldest live entry's start"
        );
    }

    /// Pack the open gesture's entry, if one is open: it is over. A gesture
    /// that ended where it started records nothing.
    fn seal(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        if !open.step.is_noop() {
            self.pack(slice::from_ref(&open.step));
        }
    }

    /// Append `steps` as the newest applied entry, then trim to the budget.
    fn pack(&mut self, steps: &[UndoStep]) {
        let range = Self::append_steps(&mut self.actions, steps);
        self.entries.push_back(range);
        self.cursor = self.entries.len();
        self.trim_to_limit();
    }

    fn append_steps(buffer: &mut Vec<u8>, steps: &[UndoStep]) -> Range<usize> {
        assert!(
            !steps.is_empty(),
            "undo stack should not store empty step batches"
        );
        let start = buffer.len();
        common::serialize_into(steps, SerdeFormat::Bitcode, buffer)
            .expect("bitcode serialize of in-memory undo steps is infallible");
        let end = buffer.len();
        start..end
    }

    fn deserialize_steps(bytes: &[u8]) -> Vec<UndoStep> {
        common::deserialize(bytes, SerdeFormat::Bitcode).unwrap()
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
pub(crate) mod internals {
    use crate::core::edit::action_stack::ActionStack;

    impl ActionStack {
        /// Whether [`ActionStack::undo`] would take an entry back: a packed one,
        /// or the open gesture unless sealing it would drop it.
        pub(crate) fn can_undo(&self) -> bool {
            self.cursor > 0 || self.open.as_ref().is_some_and(|open| !open.step.is_noop())
        }
    }
}

#[cfg(test)]
mod tests;
