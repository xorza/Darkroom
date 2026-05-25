//! Bitcode-packed undo/redo stack ported from `darkroom-egui`.
//!
//! Two flat byte buffers (`undo_actions` / `redo_actions`) with per-entry
//! ranges, instead of `VecDeque<Vec<UndoStep>>`. Keeps the entire
//! history in two allocations regardless of entry count, makes
//! `trim_to_limit` a single `Vec::drain` of the packed prefix, and
//! avoids per-field allocator churn that the naive enum-storage form
//! incurred (e.g. `RemoveNode` carries a full `Node` plus its captured
//! `bindings` / `subscriptions` wiring).

use std::fmt::Debug;
use std::ops::Range;

use common::SerdeFormat;

use crate::document::{Document, GraphRef};
use crate::intent::{self, GestureKey, UndoStep};

#[derive(Debug)]
struct UndoEntry {
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
struct RedoEntry {
    range: Range<usize>,
    target: GraphRef,
}

#[derive(Debug)]
pub struct ActionStack {
    undo_actions: Vec<u8>,
    redo_actions: Vec<u8>,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<RedoEntry>,
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

    /// Push a batch of just-applied steps that mutated `target`. `steps`
    /// is a single undo entry — undoing/redoing replays the whole batch
    /// atomically against `target`'s graph+view.
    pub fn push_current(&mut self, target: GraphRef, steps: &[UndoStep]) {
        if steps.is_empty() {
            return;
        }
        self.redo_actions.clear();
        self.redo_stack.clear();

        // Gesture coalescing: if this push is a single step matching the
        // previous entry's cached gesture_key *on the same graph*, merge
        // in place — keep the existing "from" half, replace the "to"
        // half. Cross-frame zoom/pan collapses to one undo step.
        if steps.len() == 1
            && let Some(key) = intent::gesture_key(&steps[0])
            && self.try_merge_with_last(&steps[0], key, target)
        {
            return;
        }

        let range = Self::append_steps(&mut self.undo_actions, steps, &mut self.temp_buffer);
        let gesture_key = if steps.len() == 1 {
            intent::gesture_key(&steps[0])
        } else {
            None
        };
        self.undo_stack.push(UndoEntry {
            range,
            gesture_key,
            target,
        });
        self.trim_to_limit();
    }

    pub fn undo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        let bytes = Self::slice_bytes(&self.undo_actions, &entry.range);
        let steps = Self::deserialize_steps(bytes, &mut self.temp_buffer);
        // `revert_step` resolves the right graph+view from `target` and
        // no-ops if it's gone (subgraph deleted) — the entry still moves
        // onto the redo stack so the two halves stay paired.
        for step in steps.iter().rev() {
            intent::revert_step(step, doc, entry.target);
            on_step(step);
        }
        let redo_range = Self::append_bytes(&mut self.redo_actions, bytes);
        self.redo_stack.push(RedoEntry {
            range: redo_range,
            target: entry.target,
        });
        Self::pop_tail_actions(&mut self.undo_actions, &entry.range);

        true
    }

    pub fn redo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        let bytes = Self::slice_bytes(&self.redo_actions, &entry.range);
        let steps = Self::deserialize_steps(bytes, &mut self.temp_buffer);
        let gesture_key = if steps.len() == 1 {
            intent::gesture_key(&steps[0])
        } else {
            None
        };
        for step in steps.iter() {
            intent::apply_step(step, doc, entry.target);
            on_step(step);
        }
        let undo_range = Self::append_bytes(&mut self.undo_actions, bytes);
        self.undo_stack.push(UndoEntry {
            range: undo_range,
            gesture_key,
            target: entry.target,
        });
        Self::pop_tail_actions(&mut self.redo_actions, &entry.range);

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

    fn try_merge_with_last(
        &mut self,
        new_step: &UndoStep,
        key: GestureKey,
        target: GraphRef,
    ) -> bool {
        let Some(last) = self.undo_stack.last() else {
            return false;
        };
        if last.gesture_key != Some(key) || last.target != target {
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
        // half of the incoming step. Add a match arm here when
        // introducing a new gesture variant in `gesture_key`. The
        // `gesture_key == Some(key)` check above guarantees the pair is
        // the same variant *and*, for `NodeDrag`, the same node id.
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
            (UndoStep::SwitchTab { from, .. }, UndoStep::SwitchTab { to, .. }) => {
                UndoStep::SwitchTab {
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
            target,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::intent::{Intent, apply_step, build_step};
    use scenarium::prelude::SubgraphId;
    use scenarium::testing::test_graph;

    /// A document with three tab slots so `active` can move 0→1→2. The
    /// extra slots reuse `Main` — the switch step only reads/writes
    /// `active`, never the tab graphs, so degenerate slots are fine here.
    fn doc_with_three_tabs() -> Document {
        let mut doc: Document = test_graph().into();
        doc.tabs = vec![GraphRef::Main, GraphRef::Main, GraphRef::Main];
        doc.active = 0;
        doc
    }

    /// Commit a tab switch through the real intent path and push it.
    fn switch_to(stack: &mut ActionStack, doc: &mut Document, to: usize) {
        let step = build_step(Intent::SwitchTab { to }, doc, GraphRef::Main).unwrap();
        apply_step(&step, doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
    }

    #[test]
    fn consecutive_switches_coalesce_into_one_undo() {
        let mut doc = doc_with_three_tabs();
        let mut stack = ActionStack::new(100);

        switch_to(&mut stack, &mut doc, 1);
        switch_to(&mut stack, &mut doc, 2);
        assert_eq!(doc.active, 2, "active follows the latest switch");

        // The two switches merged: a single undo jumps straight back to
        // the pre-burst tab (0), not to the intermediate 1.
        assert!(stack.undo(&mut doc, &mut |_| {}));
        assert_eq!(doc.active, 0, "one undo reverts the whole switch burst");

        // No second entry survived the merge.
        assert!(
            !stack.undo(&mut doc, &mut |_| {}),
            "the burst collapsed to exactly one entry"
        );
    }

    #[test]
    fn redo_replays_the_merged_switch() {
        let mut doc = doc_with_three_tabs();
        let mut stack = ActionStack::new(100);

        switch_to(&mut stack, &mut doc, 1);
        switch_to(&mut stack, &mut doc, 2);
        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.active, 0);

        assert!(stack.redo(&mut doc, &mut |_| {}));
        assert_eq!(doc.active, 2, "redo restores the merged switch target");
    }

    #[test]
    fn switch_does_not_merge_across_an_intervening_edit() {
        // A non-switch entry between two switches breaks the gesture, so
        // the second switch starts a fresh, separately-undoable entry.
        let mut doc = doc_with_three_tabs();
        let mut stack = ActionStack::new(100);

        switch_to(&mut stack, &mut doc, 1);

        // Intervening selection edit (a real change, so not a no-op).
        let node_id = doc.graph.iter().next().unwrap().id;
        let mut want = std::collections::BTreeSet::new();
        want.insert(node_id);
        let sel = build_step(Intent::SetSelection { to: want }, &doc, GraphRef::Main).unwrap();
        apply_step(&sel, &mut doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[sel]);

        switch_to(&mut stack, &mut doc, 2);
        assert_eq!(doc.active, 2);

        // First undo reverts only the second switch (2 → 1); it didn't
        // merge into the first because the selection edit broke the run.
        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.active, 1, "switch after an edit is its own entry");
    }

    /// Three tabs with distinct `Local` targets so a close at a given
    /// index is observable in the surviving tab list. `CloseTab` is
    /// document-global (it only touches `tabs`/`active`, never resolves a
    /// graph), so the fabricated subgraph ids need no backing graph.
    fn doc_with_distinct_tabs() -> Document {
        let a: SubgraphId = "11111111-1111-1111-1111-111111111111".into();
        let b: SubgraphId = "22222222-2222-2222-2222-222222222222".into();
        let mut doc: Document = test_graph().into();
        doc.tabs = vec![GraphRef::Main, GraphRef::Local(a), GraphRef::Local(b)];
        doc.active = 0;
        doc
    }

    /// Commit a tab close through the real intent path and push it.
    /// Returns `false` when `build_step` dropped the intent (Main /
    /// out-of-range), so callers can assert the guard.
    fn close_at(stack: &mut ActionStack, doc: &mut Document, index: usize) -> bool {
        let Some(step) = build_step(Intent::CloseTab { index }, doc, GraphRef::Main) else {
            return false;
        };
        apply_step(&step, doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
        true
    }

    #[test]
    fn close_is_dropped_for_main_or_out_of_range() {
        let mut doc = doc_with_distinct_tabs();
        let mut stack = ActionStack::new(100);
        // Main (index 0) is never closable; index 3 is past the end.
        assert!(!close_at(&mut stack, &mut doc, 0), "Main must not close");
        assert!(!close_at(&mut stack, &mut doc, 3), "OOB index must drop");
        assert_eq!(doc.tabs.len(), 3, "no tab removed");
    }

    #[test]
    fn close_then_undo_restores_tab_and_active() {
        let mut doc = doc_with_distinct_tabs();
        let b = doc.tabs[2];
        let mut stack = ActionStack::new(100);
        doc.active = 2; // viewing the tab we're about to close

        assert!(close_at(&mut stack, &mut doc, 2));
        // Tab gone; active clamped from 2 into the new range [0, 1].
        assert_eq!(doc.tabs.len(), 2);
        assert_eq!(doc.active, 1, "active clamped after closing the last tab");

        // Undo reinserts the closed tab at its index and restores active.
        assert!(stack.undo(&mut doc, &mut |_| {}));
        assert_eq!(doc.tabs.len(), 3);
        assert_eq!(doc.tabs[2], b, "closed tab restored at its original index");
        assert_eq!(doc.active, 2, "active restored to the pre-close value");
    }

    #[test]
    fn close_left_of_cursor_shifts_active_down() {
        // Closing a tab to the left of the active one shifts the cursor
        // left by one so it keeps pointing at the same graph.
        let mut doc = doc_with_distinct_tabs();
        let b = doc.tabs[2];
        let mut stack = ActionStack::new(100);
        doc.active = 2;

        assert!(close_at(&mut stack, &mut doc, 1));
        assert_eq!(doc.tabs.len(), 2);
        // Old index 2 (`b`) is now at index 1, and active followed it.
        assert_eq!(doc.active, 1);
        assert_eq!(doc.tabs[1], b);

        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.active, 2, "active restored across the reinsert");
        assert_eq!(doc.tabs.len(), 3);
    }

    #[test]
    fn close_redo_replays() {
        let mut doc = doc_with_distinct_tabs();
        let mut stack = ActionStack::new(100);
        doc.active = 1;

        close_at(&mut stack, &mut doc, 1);
        assert_eq!(doc.tabs.len(), 2);
        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.tabs.len(), 3);

        assert!(stack.redo(&mut doc, &mut |_| {}));
        assert_eq!(doc.tabs.len(), 2, "redo re-closes the tab");
        assert_eq!(doc.active, 1);
    }

    #[test]
    fn consecutive_closes_do_not_coalesce() {
        // Each close is its own undo entry — two closes need two undos.
        let mut doc = doc_with_distinct_tabs();
        let mut stack = ActionStack::new(100);

        close_at(&mut stack, &mut doc, 2);
        close_at(&mut stack, &mut doc, 1);
        assert_eq!(doc.tabs.len(), 1, "both subgraph tabs closed");

        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.tabs.len(), 2, "first undo restores one tab");
        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.tabs.len(), 3, "second undo restores the other");
    }
}
