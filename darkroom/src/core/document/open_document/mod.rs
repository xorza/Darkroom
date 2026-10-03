//! The document currently open in a frontend, with its persistence path and
//! the history of edits made to it.
//!
//! Every mutation of the document goes through this type: a frontend hands it
//! intents and it decides what they mean, records them, and lands their
//! outcomes. Keeping the history here rather than on a frontend is what makes
//! that true — the two are one unit, replaced together when a different file
//! is opened.

use std::path;
use std::path::{Path, PathBuf};

use crate::core::document::Document;
use crate::core::document::graph_revision::GraphRevision;
use crate::core::edit::action_stack::ActionStack;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::document_request::DocumentRequest;
use crate::core::edit::error::MalformedIntent;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::relayout::Relayout;
use crate::core::edit::step::undo_step::UndoStep;
use crate::core::io::document::{self, DocumentLoadError, DocumentSaveError};
use crate::core::io::preferences::Preferences;
use crate::core::status::{StatusFamily, StatusLog};
use scenarium::Library;

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
    retyped: bool,
}

impl StepSignals {
    fn fold(&mut self, step: &UndoStep) {
        self.geometry_stale |= step.invalidates_cached_geometry();
        self.dirtied |= step.dirties_document();
        self.retyped |= step.retypes_outputs();
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
    /// [`UndoStep::dirties_document`](crate::core::edit::step::undo_step::UndoStep::dirties_document).
    ///
    /// It can read "dirty" after an undo returns the document to its saved
    /// state — the safe direction (prompt rather than silently discard).
    pub(crate) dirty: bool,
    /// The state of the graph its output types are resolved from; moved by
    /// every edit, undo or redo that can retype an output.
    revision: GraphRevision,
    /// This document's undo history. Beside the document rather than on a
    /// frontend because it *is* document state — opening another file
    /// replaces the pair, and no UI can outlive one and inherit the other's
    /// history.
    history: ActionStack,
    /// The steps of the batch [`Self::commit`] is recording, kept so a frame's
    /// edits allocate no list once it has grown. Empty between commits.
    batch: Vec<UndoStep>,
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
            revision: GraphRevision::fresh(),
            history: ActionStack::new(UNDO_HISTORY_BYTES),
            batch: Vec::new(),
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
            .expect("a widget built a malformed intent")
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
    /// most often orphans a tab — an undo replaying a node removal — never
    /// touches the queue, so a frame whose only edit was Ctrl+Z would skip it.
    /// Not `commit`, because closing a tab is a frontend policy about what is
    /// on screen, and an edit applied outside a frame (a dialog result) has no
    /// business acting on it.
    #[must_use]
    pub(crate) fn drain_requests(&mut self, requests: &mut DocumentQueue) -> Relayout {
        // Usually nothing is queued, and the commit is what allocates.
        let relayout = if requests.is_empty() {
            Relayout::NotNeeded
        } else {
            self.commit(requests.drain())
                .expect("a widget built a malformed intent")
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
    /// is stale — which yields no step and is dropped. A [`MalformedIntent`]
    /// is therefore our own bug, and comes back as one rather than going to a
    /// log nobody reads. Both callers unwrap it for now, so it still fails
    /// loudly in every build.
    ///
    /// The batch stops at the first malformed intent: the steps before it are
    /// already applied to the document but never reach the history, so an
    /// `Err` leaves the document mutated and unundoable. That is survivable
    /// only because the error is unreachable — a caller that ever *handles*
    /// this has to stage the batch first.
    ///
    /// No-op and stale intents are dropped per-intent, and an empty batch
    /// records nothing. A *run* of intents becomes one undo entry, and a batch
    /// that is one frame of a held gesture folds into that gesture's entry, so
    /// a drag held for N frames is still one Ctrl+Z.
    ///
    /// Returns whether the batch stranded the canvas's cached geometry. The
    /// batch's other outcome — a dirtied document — is landed here; only the
    /// relayout has to travel out to whoever holds the `Ui`.
    fn commit(
        &mut self,
        queued: impl IntoIterator<Item = DocumentRequest>,
    ) -> Result<Relayout, MalformedIntent> {
        debug_assert!(self.batch.is_empty(), "the last commit left steps behind");
        let mut signals = StepSignals::default();
        // The gesture of the batch's last step; it names the batch only when
        // that step is the batch's one.
        let mut gesture = None;
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
            let frame_of = intent.gesture();
            let step = match intent.commit(&mut self.document) {
                Ok(Some(step)) => step,
                Ok(None) => continue,
                Err(malformed) => {
                    self.batch.clear();
                    return Err(malformed);
                }
            };
            signals.fold(&step);
            self.batch.push(step);
            gesture = frame_of;
        }
        let gesture = gesture.filter(|_| self.batch.len() == 1);
        self.history.push(&mut self.batch, gesture);
        Ok(self.land(&signals))
    }

    /// Replay the last entry backwards, and report whether that stranded the
    /// canvas's cached geometry. With nothing to undo it is a no-op.
    #[must_use]
    pub(crate) fn undo(&mut self) -> Relayout {
        // Folded into a value first: the replay callback runs while the
        // history is mutably borrowed, so it cannot touch `self`.
        let mut signals = StepSignals::default();
        self.history
            .undo(&mut self.document, &mut |step| signals.fold(step));
        self.land(&signals)
    }

    /// Replay the next entry forwards — the mirror of [`Self::undo`].
    #[must_use]
    pub(crate) fn redo(&mut self) -> Relayout {
        let mut signals = StepSignals::default();
        self.history
            .redo(&mut self.document, &mut |step| signals.fold(step));
        self.land(&signals)
    }

    /// Land folded [`StepSignals`]. The one place each signal's *effect* is
    /// spelled out, for both the commit path and undo/redo replay.
    ///
    /// Returns the one signal whose effect is a *call* rather than a stored
    /// flag, so it has to travel back to whoever holds the `Ui`.
    #[must_use]
    fn land(&mut self, signals: &StepSignals) -> Relayout {
        // A content edit (or an undone/redone one) leaves the doc differing
        // from the last save — barring the exact round-trip back to it, where
        // we accept a stale "dirty" rather than tracking saved state
        // precisely.
        self.dirty |= signals.dirtied;
        if signals.retyped {
            self.revision = GraphRevision::fresh();
        }
        Relayout::needed_if(signals.geometry_stale)
    }

    /// The state of the graph that output types are resolved from.
    pub(crate) fn graph_revision(&self) -> GraphRevision {
        self.revision
    }

    /// Open the document at `path`, holding each func node to the ports it was
    /// authored against in `library`: a node whose func moved its ports is
    /// refused by name, rather than loaded with its wiring on the wrong ports.
    pub(crate) fn load(path: PathBuf, library: &Library) -> Result<Self, DocumentLoadError> {
        let mut document = document::load(&path)?;
        if let Err(source) = document.graph.reconcile_signatures(library) {
            return Err(DocumentLoadError::InvalidDocument {
                path,
                source: source.into(),
            });
        }
        Ok(Self {
            document,
            path: Some(path),
            ..Self::default()
        })
    }

    /// The document a launching frontend opens: `argument` when the command
    /// line named a file, otherwise the one `preferences` remembers.
    ///
    /// A named file outranks the remembered one, reopening preference
    /// included — the user asked for that document by name — and a named file
    /// that fails to load degrades to an empty document rather than falling
    /// back to the remembered one, which would read as the file having loaded.
    /// Either way it leaves the preferences alone: a command-line document is
    /// this launch's, and saving it is what makes it the remembered one.
    ///
    /// A remembered document that fails to load is reported to `status` and
    /// forgotten in `preferences`, so the next launch starts clean instead of
    /// failing again; persisting that is the caller's.
    pub(crate) fn open_at_launch(
        argument: Option<PathBuf>,
        preferences: &mut Preferences,
        status: &mut StatusLog,
        library: &Library,
    ) -> Self {
        let Some(path) = argument else {
            return Self::load_preferred(preferences, status, library);
        };
        // Made absolute up front: the argument is relative to the shell's
        // working directory, which the file dialogs' anchor and the worker's
        // disk cache both outlive.
        let path = path::absolute(&path).unwrap_or(path);
        Self::load(path, library).unwrap_or_else(|error| {
            status.error(StatusFamily::Document, format!("load failed: {error:#}"));
            Self::default()
        })
    }

    /// The document `preferences` remembers, or an empty one when there is
    /// none or reopening is switched off.
    fn load_preferred(
        preferences: &mut Preferences,
        status: &mut StatusLog,
        library: &Library,
    ) -> Self {
        let Some(path) = preferences
            .document_path
            .clone()
            .filter(|_| preferences.load_last_document)
        else {
            return Self::default();
        };
        match Self::load(path, library) {
            Ok(open) => open,
            Err(error) => {
                status.error(StatusFamily::Document, format!("load failed: {error:#}"));
                preferences.document_path = None;
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

        /// Whether an undo would take an entry back.
        pub(crate) fn can_undo(&self) -> bool {
            self.history.can_undo()
        }
    }
}

#[cfg(test)]
mod tests;
