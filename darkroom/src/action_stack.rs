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
//! budget. One contiguous allocation regardless of entry count; no
//! per-field churn (the naive `VecDeque<Vec<UndoStep>>` form re-allocated
//! a `RemoveNode`'s `Node` + captured wiring on every entry).

use std::ops::Range;

use common::SerdeFormat;

use crate::document::{Document, GraphRef};
use crate::intent::{self, DocStep, GestureKey, GraphStep, UndoStep};

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
pub struct ActionStack {
    /// The single packed history buffer, oldest entry first.
    actions: Vec<u8>,
    /// Per-entry metadata, parallel to the packed entries in `actions`.
    entries: Vec<Entry>,
    /// Boundary between applied (`entries[..cursor]`) and undone
    /// (`entries[cursor..]`) entries. Undo decrements, redo increments, a
    /// new edit truncates the undone tail then appends.
    cursor: usize,
    /// Byte budget for `actions`. When a push overflows it the oldest
    /// entries are dropped off the front; the just-pushed entry always
    /// survives, even when it alone exceeds the budget. Bounds history by
    /// memory rather than entry count, since one entry (e.g. a
    /// `RemoveNode` carrying a whole `Node` + wiring) can dwarf many small
    /// ones.
    max_bytes: usize,
    temp_buffer: Vec<u8>,
}

impl ActionStack {
    pub fn new(max_bytes: usize) -> Self {
        assert!(max_bytes > 0, "undo history needs a positive byte budget");
        Self {
            actions: Vec::new(),
            entries: Vec::new(),
            cursor: 0,
            max_bytes,
            temp_buffer: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.actions.clear();
        self.entries.clear();
        self.cursor = 0;
    }

    /// Push a batch of just-applied steps that mutated `target`. `steps`
    /// is a single undo entry — undoing/redoing replays the whole batch
    /// atomically against `target`'s graph+view.
    pub fn push_current(&mut self, target: GraphRef, steps: &[UndoStep]) {
        if steps.is_empty() {
            return;
        }
        // A fresh edit makes the undone tail unreachable; drop it.
        self.discard_redo();

        // A gesture key only exists for single-step batches; a multi-step
        // batch (e.g. a breaker swipe) is never coalesced.
        let gesture_key = match steps {
            [step] => intent::gesture_key(step),
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

        let range = Self::append_steps(&mut self.actions, steps, &mut self.temp_buffer);
        self.entries.push(Entry {
            range,
            gesture_key,
            target,
        });
        self.cursor = self.entries.len();
        self.trim_to_limit();
    }

    pub fn undo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        // `revert_step` resolves the right graph+view from `target` and
        // no-ops if it's gone (subgraph deleted). The entry stays in the
        // buffer — it just moved into the redoable region.
        let (range, target) = self.entry_at(self.cursor);
        let steps = Self::deserialize_steps(
            Self::slice_bytes(&self.actions, &range),
            &mut self.temp_buffer,
        );
        for step in steps.iter().rev() {
            intent::revert_step(step, doc, target);
            on_step(step);
        }
        true
    }

    pub fn redo(&mut self, doc: &mut Document, on_step: &mut dyn FnMut(&UndoStep)) -> bool {
        if self.cursor == self.entries.len() {
            return false;
        }
        let (range, target) = self.entry_at(self.cursor);
        let steps = Self::deserialize_steps(
            Self::slice_bytes(&self.actions, &range),
            &mut self.temp_buffer,
        );
        for step in steps.iter() {
            intent::apply_step(step, doc, target);
            on_step(step);
        }
        self.cursor += 1;
        true
    }

    /// `(range, target)` of entry `i`, cloned so callers can then borrow
    /// `actions` / `temp_buffer` without holding the `entries` borrow.
    fn entry_at(&self, i: usize) -> (Range<usize>, GraphRef) {
        let e = &self.entries[i];
        (e.range.clone(), e.target)
    }

    /// Drop the redoable tail (`entries[cursor..]`) and its bytes — a new
    /// edit makes them unreachable. Truncations from the end, so no
    /// memmove.
    fn discard_redo(&mut self) {
        if self.cursor < self.entries.len() {
            let cut = self.entries[self.cursor].range.start;
            self.actions.truncate(cut);
            self.entries.truncate(self.cursor);
        }
    }

    fn trim_to_limit(&mut self) {
        // Drop the oldest entries one at a time until the packed buffer
        // fits the budget — minimal history loss. Always keep the last
        // (just-pushed) entry. Runs only right after a push, where every
        // entry is applied (`cursor == entries.len()`), so each front
        // drop also steps the cursor down.
        while self.entries.len() > 1 && self.actions.len() > self.max_bytes {
            let drop_end = self.entries[0].range.end;
            self.entries.remove(0);
            self.actions.drain(0..drop_end);
            for entry in &mut self.entries {
                entry.range.start -= drop_end;
                entry.range.end -= drop_end;
            }
            self.cursor -= 1;
        }
    }

    fn try_merge_with_last(
        &mut self,
        new_step: &UndoStep,
        key: GestureKey,
        target: GraphRef,
    ) -> bool {
        // `discard_redo` ran first, so the last entry is the last applied
        // one and its bytes are the buffer tail.
        let Some(last) = self.entries.last() else {
            return false;
        };
        if last.gesture_key != Some(key) || last.target != target {
            return false;
        }
        let last_range = last.range.clone();
        let last_bytes = Self::slice_bytes(&self.actions, &last_range);
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
                UndoStep::Graph(GraphStep::SetViewport {
                    from_pan,
                    from_scale,
                    ..
                }),
                UndoStep::Graph(GraphStep::SetViewport {
                    to_pan, to_scale, ..
                }),
            ) => UndoStep::Graph(GraphStep::SetViewport {
                from_pan: *from_pan,
                from_scale: *from_scale,
                to_pan: *to_pan,
                to_scale: *to_scale,
            }),
            (
                UndoStep::Graph(GraphStep::MoveNode { node_id, from, .. }),
                UndoStep::Graph(GraphStep::MoveNode { to, .. }),
            ) => UndoStep::Graph(GraphStep::MoveNode {
                node_id: *node_id,
                from: *from,
                to: *to,
            }),
            (
                UndoStep::Doc(DocStep::SwitchTab { from, .. }),
                UndoStep::Doc(DocStep::SwitchTab { to, .. }),
            ) => UndoStep::Doc(DocStep::SwitchTab {
                from: *from,
                to: *to,
            }),
            _ => return false,
        };

        // The last entry is the buffer tail — truncate it off and
        // re-append the merged step in place.
        self.actions.truncate(last_range.start);
        self.entries.pop();
        let range = Self::append_steps(&mut self.actions, &[merged], &mut self.temp_buffer);
        self.entries.push(Entry {
            range,
            gesture_key: Some(key),
            target,
        });
        self.cursor = self.entries.len();
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
        let mut stack = ActionStack::new(1 << 20);

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
        let mut stack = ActionStack::new(1 << 20);

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
        let mut stack = ActionStack::new(1 << 20);

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
        let mut stack = ActionStack::new(1 << 20);
        // Main (index 0) is never closable; index 3 is past the end.
        assert!(!close_at(&mut stack, &mut doc, 0), "Main must not close");
        assert!(!close_at(&mut stack, &mut doc, 3), "OOB index must drop");
        assert_eq!(doc.tabs.len(), 3, "no tab removed");
    }

    #[test]
    fn close_then_undo_restores_tab_and_active() {
        let mut doc = doc_with_distinct_tabs();
        let b = doc.tabs[2];
        let mut stack = ActionStack::new(1 << 20);
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
        let mut stack = ActionStack::new(1 << 20);
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
        let mut stack = ActionStack::new(1 << 20);
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
        let mut stack = ActionStack::new(1 << 20);

        close_at(&mut stack, &mut doc, 2);
        close_at(&mut stack, &mut doc, 1);
        assert_eq!(doc.tabs.len(), 1, "both subgraph tabs closed");

        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.tabs.len(), 2, "first undo restores one tab");
        stack.undo(&mut doc, &mut |_| {});
        assert_eq!(doc.tabs.len(), 3, "second undo restores the other");
    }

    #[test]
    fn history_bounded_by_byte_budget() {
        use std::collections::BTreeSet;

        let mut doc: Document = test_graph().into();
        let node = doc.graph.iter().next().unwrap().id;
        // Tiny budget so a handful of small entries overflow it.
        let mut stack = ActionStack::new(256);

        // Many distinct, non-coalescing selection edits (toggle one node
        // in/out — `from != to` each time, gesture key `None`).
        for i in 0..200 {
            let to: BTreeSet<_> = if i % 2 == 0 {
                [node].into_iter().collect()
            } else {
                BTreeSet::new()
            };
            let step = build_step(Intent::SetSelection { to }, &doc, GraphRef::Main).unwrap();
            apply_step(&step, &mut doc, GraphRef::Main);
            stack.push_current(GraphRef::Main, &[step]);
            // The single packed buffer never exceeds the budget (entries
            // are far smaller than 256 B, so no single-entry overflow).
            assert!(
                stack.actions.len() <= stack.max_bytes,
                "buffer {} exceeded budget {} after push {i}",
                stack.actions.len(),
                stack.max_bytes,
            );
        }

        // Old entries were dropped (not all 200 kept) and the newest is
        // still undoable.
        assert!(
            stack.entries.len() < 200,
            "oldest entries should have been trimmed"
        );
        assert!(
            stack.undo(&mut doc, &mut |_| {}),
            "the most recent edit stays undoable"
        );
    }

    /// A document carrying a subgraph def "S" with interface inputs
    /// `[A]` and outputs `[R]`, plus that `Local` target.
    fn doc_with_def() -> (Document, GraphRef) {
        use scenarium::data::DataType;
        use scenarium::function::{FuncInput, FuncOutput};
        use scenarium::prelude::{Graph, SubgraphDef};

        let mut doc: Document = test_graph().into();
        let def = SubgraphDef {
            id: "00000000-0000-0000-0000-0000000000bb".into(),
            name: "S".into(),
            category: "Subgraph".into(),
            graph: Graph::default(),
            inputs: vec![FuncInput {
                name: "A".into(),
                required: false,
                data_type: DataType::Int,
                default_value: None,
                value_options: Vec::new(),
            }],
            outputs: vec![FuncOutput {
                name: "R".into(),
                data_type: DataType::Int,
            }],
            events: vec![],
        };
        let id = def.id;
        doc.graph.subgraphs.add(def);
        (doc, GraphRef::Local(id))
    }

    #[test]
    fn rename_boundary_port_applies_and_reverts() {
        use crate::document::BoundarySide;
        use crate::intent::revert_step;

        let (mut doc, target) = doc_with_def();
        let GraphRef::Local(def_id) = target else {
            unreachable!()
        };
        let step = build_step(
            Intent::RenameBoundaryPort {
                side: BoundarySide::Input,
                idx: 0,
                to: "alpha".into(),
            },
            &doc,
            target,
        )
        .expect("rename builds against a Local target");

        apply_step(&step, &mut doc, target);
        assert_eq!(
            doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
            "alpha"
        );

        revert_step(&step, &mut doc, target);
        assert_eq!(
            doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
            "A",
            "revert restores the captured `from` name"
        );
    }

    #[test]
    fn rename_boundary_port_renames_outputs_side() {
        use crate::document::BoundarySide;

        let (mut doc, target) = doc_with_def();
        let GraphRef::Local(def_id) = target else {
            unreachable!()
        };
        let step = build_step(
            Intent::RenameBoundaryPort {
                side: BoundarySide::Output,
                idx: 0,
                to: "result".into(),
            },
            &doc,
            target,
        )
        .unwrap();
        apply_step(&step, &mut doc, target);
        assert_eq!(
            doc.graph.subgraphs.by_key(&def_id).unwrap().outputs[0].name,
            "result"
        );
    }

    #[test]
    fn rename_boundary_port_dropped_off_local_target_or_oob() {
        use crate::document::BoundarySide;

        let (doc, target) = doc_with_def();
        // Main target has no def interface to rename.
        assert!(
            build_step(
                Intent::RenameBoundaryPort {
                    side: BoundarySide::Input,
                    idx: 0,
                    to: "x".into(),
                },
                &doc,
                GraphRef::Main,
            )
            .is_none()
        );
        // Out-of-range index on the right target also drops.
        assert!(
            build_step(
                Intent::RenameBoundaryPort {
                    side: BoundarySide::Input,
                    idx: 9,
                    to: "x".into(),
                },
                &doc,
                target,
            )
            .is_none()
        );
    }

    #[test]
    fn rename_undo_survives_interface_compaction() {
        use crate::document::BoundarySide;
        use crate::intent::revert_step;
        use scenarium::data::DataType;
        use scenarium::function::FuncInput;
        use scenarium::prelude::{Graph, SubgraphDef};

        let finput = |n: &str| FuncInput {
            name: n.into(),
            required: false,
            data_type: DataType::Int,
            default_value: None,
            value_options: Vec::new(),
        };
        let mut doc: Document = test_graph().into();
        let def = SubgraphDef {
            id: "00000000-0000-0000-0000-0000000000cc".into(),
            name: "S".into(),
            category: "Subgraph".into(),
            graph: Graph::default(),
            inputs: vec![finput("A"), finput("B")],
            outputs: vec![],
            events: vec![],
        };
        let def_id = def.id;
        doc.graph.subgraphs.add(def);
        let target = GraphRef::Local(def_id);

        // Rename inputs[1] "B" -> "beta".
        let step = build_step(
            Intent::RenameBoundaryPort {
                side: BoundarySide::Input,
                idx: 1,
                to: "beta".into(),
            },
            &doc,
            target,
        )
        .unwrap();
        apply_step(&step, &mut doc, target);
        assert_eq!(
            doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[1].name,
            "beta"
        );

        // Simulate `reconcile_boundaries` compacting after input 0 ("A")
        // was disconnected: the survivor "beta" shifts from index 1 to 0.
        doc.graph
            .subgraphs
            .by_key_mut(&def_id)
            .unwrap()
            .inputs
            .remove(0);
        assert_eq!(
            doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
            "beta"
        );

        // Undo: the step's `idx` (1) is now stale, but it resolves "beta"
        // by name at its new index 0 and restores "B" — not a no-op, not
        // the wrong slot.
        revert_step(&step, &mut doc, target);
        assert_eq!(
            doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
            "B"
        );
    }
}
