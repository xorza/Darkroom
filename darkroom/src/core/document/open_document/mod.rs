//! The document currently open in a frontend, with its persistence path and
//! the history of edits made to it.
//!
//! Every mutation of the document goes through this type: a frontend hands it
//! intents and it decides what they mean, records them, and lands their
//! outcomes. Keeping the history here rather than on a frontend is what makes
//! that true — the two are one unit, replaced together when a different file
//! is opened.

pub(crate) mod error;

use std::path::{Path, PathBuf};

use crate::core::document::Document;
use crate::core::document::open_document::error::ReplayOutcome;
use crate::core::edit::action_stack::ActionStack;
use crate::core::edit::intent::apply::commit_intent;
use crate::core::edit::intent::types::{GraphIntent, Refusal, UndoStep};
use crate::core::io::document::{self, DocumentLoadError, DocumentSaveError};
use crate::core::io::preferences::Preferences;
use crate::core::status::StatusLog;
use crate::gui::relayout::Relayout;
use crate::gui::requests::{DocumentRequest, Requests};

/// Byte budget for the undo history's packed buffer (~1 MiB). Bounds
/// memory rather than entry count — a single large edit can't be
/// undone away, but the oldest entries drop once the buffer overflows.
const UNDO_HISTORY_BYTES: usize = 1 << 20;

/// What applying one or more [`UndoStep`]s obliges the caller to do, folded
/// off the steps' own predicates. Accumulated as a value rather than written
/// straight onto [`OpenDocument`] because undo/redo replay folds its steps
/// from a callback while the history itself is mutably borrowed — so both the
/// commit path and the replay path fold into one of these and then spend it:
/// `dirtied` lands on the document, `geometry_stale` is reported up to
/// whoever holds the `Ui` and owes the relayout.
#[derive(Default, Debug)]
struct StepSignals {
    geometry_stale: bool,
    dirtied: bool,
}

impl StepSignals {
    fn fold(&mut self, step: &UndoStep) {
        self.geometry_stale |= step.invalidates_cached_geometry();
        self.dirtied |= step.dirties_document();
    }
}

#[derive(Debug)]
pub(crate) struct OpenDocument {
    pub(crate) document: Document,
    pub(crate) path: Option<PathBuf>,
    /// Whether `document` differs from what is at `path` — the pair that
    /// gives the flag its meaning, which is why it lives here rather than on
    /// a frontend. Set by any content-changing edit (new edits, undo/redo
    /// replay, direct graph mutations), cleared by [`Self::save_to`]. Pure
    /// navigation (camera, selection, pane arrangement) leaves it alone; see
    /// [`UndoStep::dirties_document`](crate::core::edit::intent::types::UndoStep::dirties_document).
    ///
    /// It can read "dirty" after an undo returns the document to its saved
    /// state — the safe direction (prompt rather than silently discard).
    pub(crate) dirty: bool,
    /// This document's undo history. Beside the document rather than on a
    /// frontend because it *is* document state — opening another file
    /// replaces the pair, and no UI can outlive one and inherit the other's
    /// history.
    history: ActionStack,
}

impl Default for OpenDocument {
    /// An empty document with a fresh history. Hand-written rather than
    /// derived because the history needs its byte budget, which has no
    /// meaningful zero.
    fn default() -> Self {
        Self {
            document: Document::default(),
            path: None,
            dirty: false,
            history: ActionStack::new(UNDO_HISTORY_BYTES),
        }
    }
}

impl OpenDocument {
    /// Apply a single `intent` and record it as its own undo entry. For edits
    /// raised outside a frame's drain — e.g. a file-picker result handled
    /// after the record. No-ops (and self-cancelling steps) are dropped, like
    /// the in-frame drain.
    #[must_use]
    pub(crate) fn apply_edit(&mut self, intent: GraphIntent) -> Relayout {
        self.commit([DocumentRequest::Graph(intent)])
    }

    /// Take everything `requests` holds for this document, apply it, and leave
    /// the document self-consistent.
    ///
    /// Called three times a frame — after the navigation scan, the prepass,
    /// and the record — so a request raised in one phase lands before the next
    /// reads the document.
    ///
    /// The reconcile is *not* inside the empty-queue fast path, and not inside
    /// [`Self::commit`] either. Not the fast path, because the mutation that
    /// most often orphans a tab — an undo replaying a `RemoveNode` — never
    /// touches the queue, so a frame whose only edit was Ctrl+Z would skip it.
    /// Not `commit`, because closing a tab is a frontend policy about what is
    /// on screen, and an edit applied outside a frame (a dialog result) has no
    /// business acting on it.
    #[must_use]
    pub(crate) fn drain_requests(&mut self, requests: &mut Requests) -> Relayout {
        // Usually nothing is queued, and the commit is what allocates.
        let relayout = if requests.is_empty() {
            Relayout::NotNeeded
        } else {
            self.commit(requests.drain_document())
        };
        // A tab whose node is gone can't stay open. Cheap when nothing died —
        // `reconcile_with_graph` scans the tab list and returns.
        self.document.reconcile_with_graph();
        relayout
    }

    /// Apply `queued`, each request according to its tier: a graph edit is
    /// built, applied, and recorded; a view op goes straight to the layout.
    /// The app tier never reaches here — it stays queued for the shell.
    ///
    /// Nothing raised here can legitimately be malformed. Widgets read every
    /// identity they emit out of the live document, so the worst they build
    /// is stale — which refuses [`Refusal::Quiet`]ly and is dropped. A
    /// [`Refusal::Invalid`] is therefore our own bug, and it panics in every
    /// build rather than going to a log nobody reads.
    ///
    /// No-op and stale intents are dropped per-intent, and an empty batch
    /// records nothing. A *run* of intents becomes one undo entry, so a
    /// gesture that emits N of them is still one Ctrl+Z.
    ///
    /// Returns whether the batch stranded the canvas's cached geometry. The
    /// batch's other outcome — a dirtied document — is landed here; only the
    /// relayout has to travel out to whoever holds the `Ui`.
    #[must_use]
    fn commit(&mut self, queued: impl IntoIterator<Item = DocumentRequest>) -> Relayout {
        let mut batch = Vec::new();
        let mut signals = StepSignals::default();
        for item in queued {
            let intent = match item {
                DocumentRequest::Graph(intent) => intent,
                // Applied straight to the layout: pane arrangement is
                // navigation, so it records no step, raises no signal, and
                // breaks no run of graph edits around it.
                DocumentRequest::View(op) => {
                    self.document.layout.apply(op);
                    continue;
                }
            };
            let step = match commit_intent(intent, &mut self.document) {
                Ok(step) => step,
                Err(Refusal::Quiet) => continue,
                Err(Refusal::Invalid(reason)) => {
                    panic!("a widget built a malformed intent: {reason}")
                }
            };
            signals.fold(&step);
            batch.push(step);
        }
        self.history.push_current(&batch);
        batch.clear();
        self.land(signals)
    }

    /// Replay the last entry backwards. Reports whether the canvas's cached
    /// geometry was stranded; `took` says whether there was an entry at all.
    #[must_use]
    pub(crate) fn undo(&mut self) -> ReplayOutcome {
        // Folded into a value first: the replay callback runs while the
        // history is mutably borrowed, so it cannot touch `self`.
        let mut signals = StepSignals::default();
        let took = self
            .history
            .undo(&mut self.document, &mut |step| signals.fold(step));
        ReplayOutcome {
            took,
            relayout: self.land(signals),
        }
    }

    /// Replay the next entry forwards — the mirror of [`Self::undo`].
    #[must_use]
    pub(crate) fn redo(&mut self) -> ReplayOutcome {
        let mut signals = StepSignals::default();
        let took = self
            .history
            .redo(&mut self.document, &mut |step| signals.fold(step));
        ReplayOutcome {
            took,
            relayout: self.land(signals),
        }
    }

    /// Land folded [`StepSignals`]. The one place each signal's *effect* is
    /// spelled out, for both the commit path and undo/redo replay.
    ///
    /// Returns the one signal whose effect is a *call* rather than a stored
    /// flag, so it has to travel back to whoever holds the `Ui`.
    #[must_use]
    fn land(&mut self, signals: StepSignals) -> Relayout {
        // A content edit (or an undone/redone one) leaves the doc differing
        // from the last save — barring the exact round-trip back to it, where
        // we accept a stale "dirty" rather than tracking saved state
        // precisely.
        self.dirty |= signals.dirtied;
        Relayout::needed_if(signals.geometry_stale)
    }

    pub(crate) fn load(path: PathBuf) -> Result<Self, DocumentLoadError> {
        let document = document::load(&path)?;
        Ok(Self {
            document,
            path: Some(path),
            ..Self::default()
        })
    }

    /// The document a launching frontend restores: the one `preferences`
    /// remembers, or an empty one when there is none or reopening is
    /// switched off. A failed load is reported to `status` and forgets the
    /// remembered path, so the next launch starts clean instead of failing
    /// again — which is why this takes the preferences by `&mut` and
    /// persists them.
    pub(crate) fn load_preferred(preferences: &mut Preferences, status: &mut StatusLog) -> Self {
        Self::load_preferred_with(preferences, status, Preferences::save)
    }

    /// [`Self::load_preferred`] with the preferences write injected, so a
    /// test can drive the path where forgetting the bad document fails too.
    fn load_preferred_with(
        preferences: &mut Preferences,
        status: &mut StatusLog,
        save_preferences: impl FnOnce(&Preferences) -> Result<(), String>,
    ) -> Self {
        let Some(path) = preferences
            .document_path
            .clone()
            .filter(|_| preferences.load_last_document)
        else {
            return Self::default();
        };
        match Self::load(path) {
            Ok(open) => open,
            Err(error) => {
                status.error(format!("load failed: {error:#}"));
                preferences.document_path = None;
                if let Err(error) = save_preferences(preferences) {
                    status.error(error);
                }
                Self::default()
            }
        }
    }

    /// Write the document to `path` and adopt it. Clears
    /// [`dirty`](Self::dirty) — only on success, so a failed save leaves the
    /// unsaved work still flagged.
    pub(crate) fn save_to(&mut self, path: &Path) -> Result<(), DocumentSaveError> {
        document::save(&self.document, path)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::document::Document;
    use crate::core::document::open_document::OpenDocument;

    impl OpenDocument {
        /// An unsaved document over `document`, with a fresh history — the
        /// shape a fixture wants, since `history` is private and `Default`
        /// cannot be spread across it from outside this module.
        pub(crate) fn over(document: Document) -> Self {
            Self {
                document,
                ..Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests;
