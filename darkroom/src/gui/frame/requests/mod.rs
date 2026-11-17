//! The frame's pending requests: everything a UI surface asked for, in the
//! order it asked.

use std::collections::VecDeque;
use std::mem;

use common::ErrorChain;
use palantir::{DockOperation, Ui};
use scenarium::{CompoundId, GraphId, InstancePath, ProgramId};

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::app_command::AppCommand;

/// A frame's requests, in the order they were raised.
///
/// A surface pushes and moves on: the push methods say nothing about which level picks the
/// request up. Behind them the tiers are stored apart, one queue per taker — [`Self::document`]
/// for the open document, [`Self::pop_app`] for the shell — so neither steps over a tier its
/// queue cannot hold.
///
/// The document tier is a graph edit or a dock op; see
/// [`DocumentRequest`](crate::core::edit::document_request::DocumentRequest).
/// The app tier is an [`AppCommand`], state the document does not own: left
/// queued by the document's drain and taken by `App` once the pass is over,
/// because every one needs `&mut App`, a blocking dialog, or both. A surface
/// picks the tier by what it is asking for, never by where it sits or when it
/// runs — the menu bar raises all three.
///
/// Nothing is dropped or reordered *within* a tier. Across tiers there is no order to keep: the
/// app tier runs after the whole pass, not interleaved with the document's.
///
/// It also holds what the frame owes palantir's layout engine, so any level that strands the
/// canvas's cross-frame geometry says so where it holds this queue, and `App` spends it once. A
/// debt outlives the drains, and goes from the input stage to the record that spends it. Refused: a
/// value each level returns, which came up six levels by hand, and which a test app could drop.
#[derive(Debug, Default)]
pub(crate) struct Requests {
    document: DocumentQueue,
    app: VecDeque<AppCommand>,
    /// Whether anything stranded the canvas's cross-frame geometry since the last spend.
    relayout_owed: bool,
    /// Whether an edit changed what a run reads since the last spend: what Run on Change runs
    /// for, once a frame however many edits landed.
    run_owed: bool,
}

impl Requests {
    /// A fresh id for a gesture that starts now; see [`GestureId`].
    pub(crate) const fn open_gesture(&mut self) -> GestureId {
        self.document.open_gesture()
    }

    /// Queue an edit of `graph`.
    pub(crate) fn push_graph(&mut self, graph: GraphId, intent: GraphIntent) {
        self.document.push_graph(graph, intent);
    }

    /// Queue every edit of `graph` `iter` yields.
    pub(crate) fn extend_graph(
        &mut self,
        graph: GraphId,
        iter: impl IntoIterator<Item = GraphIntent>,
    ) {
        self.document.extend_graph(graph, iter);
    }

    /// Queue every edit of `graph` `iter` yields as part of the held `gesture`; see
    /// [`DocumentQueue::extend_graph_in`].
    pub(crate) fn extend_graph_in(
        &mut self,
        gesture: GestureId,
        graph: GraphId,
        iter: impl IntoIterator<Item = GraphIntent>,
    ) {
        self.document.extend_graph_in(gesture, graph, iter);
    }

    /// Queue the cancel of the held `gesture` on `graph`; see [`DocumentQueue::push_cancel`].
    pub(crate) fn push_cancel(
        &mut self,
        gesture: GestureId,
        graph: GraphId,
        otherwise: GraphIntent,
    ) {
        self.document.push_cancel(gesture, graph, otherwise);
    }

    /// Queue a mutation of the pane arrangement.
    pub(crate) fn push_view(&mut self, op: DockOperation<TabRef>) {
        self.document.push_view(op);
    }

    /// Queue an edit of the set of graphs.
    pub(crate) fn push_compound(&mut self, intent: CompoundIntent) {
        self.document.push_compound(intent);
    }

    /// Queue bringing `tab` into view, where it is open or in its home.
    pub(crate) fn push_show(&mut self, tab: TabRef) {
        self.document.push_show(tab);
    }

    /// Queue making `program_id` the active preview, its viewer shown.
    pub(crate) fn push_show_preview(&mut self, program_id: ProgramId) {
        self.document.push_show_preview(program_id);
    }

    /// Queue the entry into `compound`'s pane, shown through `through`.
    pub(crate) fn push_enter(&mut self, compound: CompoundId, through: Option<InstancePath>) {
        self.document.push_enter(compound, through);
    }

    /// Queue a side effect for `App` to run after the pass.
    pub(crate) fn push_app(&mut self, command: AppCommand) {
        self.app.push_back(command);
    }

    /// The document's tier, for its drain. The app tier stays queued for
    /// [`Self::pop_app`]: the document drains three times a frame, and app
    /// commands come out at the end, the only time there is an `&mut App` to
    /// run them with.
    pub(crate) const fn document(&mut self) -> &mut DocumentQueue {
        &mut self.document
    }

    /// Take the next app-tier command, in the order raised.
    ///
    /// One at a time, not an iterator: running a command needs `&mut App`, which owns the queue,
    /// so an iterator would hold it borrowed for the whole loop. A `VecDeque`, as
    /// `Vec::remove(0)` is O(n) and popping the back runs them in reverse.
    pub(crate) fn pop_app(&mut self) -> Option<AppCommand> {
        self.app.pop_front()
    }

    /// Owe a relayout: something remeasured, as a canvas appeared or a node has no size yet, so the
    /// cached geometry read next frame would be stale.
    pub(crate) const fn owe_relayout(&mut self) {
        self.relayout_owed = true;
    }

    /// Take what an edit reported: the relayout it owes, and the refusal the status bar names.
    pub(crate) fn land(&mut self, facts: EditFacts) {
        self.owe_after(facts);
        if let Some(refusal) = facts.refused {
            self.push_app(AppCommand::Warn(ErrorChain::new(&refusal).to_string()));
        }
        if facts.dropped > 0 {
            self.push_app(AppCommand::Warn(format!(
                "the interface edit took out {} binding(s) its ports no longer hold",
                facts.dropped
            )));
        }
        if facts.ghosted > 0 {
            self.push_app(AppCommand::Warn(format!(
                "the update kept {} binding(s) its ports no longer hold as ghosts",
                facts.ghosted
            )));
        }
    }

    /// Owe what the edit that reported `facts` strands: a relayout if it strands the geometry,
    /// a run if it changed what a run reads.
    pub(crate) const fn owe_after(&mut self, facts: EditFacts) {
        self.relayout_owed |= facts.strands_geometry();
        self.run_owed |= facts.program;
    }

    /// Take the run owed: `true` once after any number of edits that changed what a run reads.
    pub(crate) const fn take_run(&mut self) -> bool {
        mem::replace(&mut self.run_owed, false)
    }

    /// Ask `ui` for the relayout owed, if one is: however many levels owed it, one pass answers. A
    /// wire reads its port's rect, which only a layout gives, so a node's first record cannot draw
    /// its wires: the sizing pass egui's `Grid` takes through `request_discard`.
    pub(crate) fn spend_relayout(&mut self, ui: &mut Ui) {
        if mem::take(&mut self.relayout_owed) {
            ui.request_relayout();
        }
    }

    /// Whether no tier holds a request: so between two record passes, as each drains its own.
    pub(crate) fn is_empty(&self) -> bool {
        self.document.is_empty() && self.app.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::gui::frame::requests::Requests;

    impl Requests {
        /// Whether a relayout is owed and not spent.
        pub(crate) const fn owes_relayout(&self) -> bool {
            self.relayout_owed
        }
    }
}

#[cfg(test)]
mod tests;
