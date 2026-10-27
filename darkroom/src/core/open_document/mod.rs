//! The document currently open in a frontend, with its persistence path and
//! the history of edits made to it.
//!
//! Every mutation of the document goes through this type: a frontend hands it
//! intents and it decides what they mean, records them, and lands their
//! outcomes. Keeping the history here rather than on a frontend is what makes
//! that true — the two are one unit, replaced together when a different file
//! is opened.
//!
//! # Decisions
//!
//! - **What a load cannot keep is removed and reported.** A binding or
//!   subscription the library refuses goes, and the document is dirty, so a
//!   save accepts the change. Refused: refusing the document, which then does
//!   not open; resetting the port to its default, which changes the meaning
//!   with no warning.
//! - **The report is one status line plus the log.** One warning line says how
//!   many changes the load made, and one `tracing` warning names each; the next
//!   success of the document family, such as a save, clears the line. Refused: a list in the editor, as new UI to design
//!   and build.

use std::path;
use std::path::{Path, PathBuf};

use crate::core::document::Document;
use crate::core::document::graph_revision::GraphRevision;
use crate::core::document::load_report::LoadReport;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::document_request::DocumentRequest;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::step::undo_step::UndoStep;
use crate::core::edit::undo_history::UndoHistory;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::error::{DocumentLoadError, DocumentSaveError};
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::io::document::{self, DocumentFile};
use crate::core::status_log::{StatusFamily, StatusLog};
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
/// `dirtied` lands on the document, and `facts` moves the graph revision and is reported up to
/// the frontend, which owes its view what they change.
#[derive(Default, Debug)]
struct StepSignals {
    dirtied: bool,
    facts: EditFacts,
}

impl StepSignals {
    fn fold(&mut self, step: &UndoStep) {
        self.dirtied |= step.dirties_document();
        self.facts |= step.facts();
    }
}

#[derive(Debug)]
pub(crate) struct OpenDocument {
    pub(crate) document: Document,
    pub(crate) path: Option<PathBuf>,
    /// The form of the file at `path`, which a plain save keeps; plain RON for a new document.
    pub(crate) format: DocumentFormat,
    /// Whether `document` differs from what is at `path` — the pair that gives the flag its
    /// meaning, which is why it lives here rather than on a frontend. Set by any content-changing
    /// edit (new edits, undo/redo replay, direct graph mutations), cleared by [`Self::save_to`].
    /// Pure navigation (camera, selection, pane arrangement) leaves it alone; see
    /// [`UndoStep::dirties_document`](crate::core::edit::step::undo_step::UndoStep::dirties_document).
    ///
    /// It can read "dirty" after an undo returns the document to its saved
    /// state — the safe direction (prompt rather than silently discard).
    pub(crate) dirty: bool,
    /// The state of the graph's node set; moved by every edit, undo or redo
    /// that adds or removes a node.
    revision: GraphRevision,
    /// This document's undo history. Beside the document rather than on a
    /// frontend because it *is* document state — opening another file
    /// replaces the pair, and no UI can outlive one and inherit the other's
    /// history.
    history: UndoHistory,
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
            format: DocumentFormat::default(),
            dirty: false,
            revision: GraphRevision::default(),
            history: UndoHistory::new(UNDO_HISTORY_BYTES),
            batch: Vec::new(),
        }
    }
}

/// What a launch opened, and whether the remembered document it tried failed to load, so the
/// caller forgets it and persists that, and the next launch starts clean instead of failing
/// again.
#[derive(Debug)]
pub(crate) struct Launched {
    pub(crate) open: OpenDocument,
    pub(crate) forgot_remembered: bool,
}

impl OpenDocument {
    /// Whether an undo would take an entry back.
    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Apply a single `intent` and record it as its own undo entry. For edits
    /// raised outside a frame's drain — e.g. a file-picker result handled
    /// after the record. No-ops (and self-cancelling steps) are dropped, like
    /// the in-frame drain.
    #[must_use]
    pub(crate) fn apply_edit(&mut self, intent: GraphIntent, library: &Library) -> EditFacts {
        self.commit([DocumentRequest::Graph(intent)], library)
    }

    /// Take everything `requests` holds for this document, apply it, and leave
    /// the document self-consistent.
    ///
    /// Called three times a frame — after the navigation scan, the prepass,
    /// and the record — so a request raised in one phase lands before the next
    /// reads the document.
    #[must_use]
    pub(crate) fn drain_requests(
        &mut self,
        requests: &mut DocumentQueue,
        library: &Library,
    ) -> EditFacts {
        // Usually nothing is queued, and the commit is what allocates.
        if requests.is_empty() {
            return EditFacts::NONE;
        }
        self.commit(requests.drain(), library)
    }

    /// Apply `queued`, each request according to its tier: a graph edit is
    /// built, applied, and recorded; a view op goes straight to the layout.
    /// The app tier never reaches here — it stays queued for the shell.
    ///
    /// # Panics
    ///
    /// On a malformed intent. Nothing raised here can legitimately be one:
    /// widgets read every identity they emit out of the live document, so the
    /// worst they build is stale — which yields no step and is dropped. A
    /// [`MalformedIntent`](crate::core::edit::error::MalformedIntent) is
    /// therefore our own bug, which crashes rather than going to a log nobody
    /// reads, and the steps before it, already applied, would reach no history.
    ///
    /// No-op and stale intents are dropped per-intent, and an empty batch
    /// records nothing. A *run* of intents becomes one undo entry, and a batch
    /// whose every step joins a held gesture folds into that gesture's entry,
    /// so a drag held for N frames is still one Ctrl+Z; a cancel takes the
    /// gesture's open entry back whole.
    ///
    /// Returns whether the batch stranded the canvas's cached geometry. The
    /// batch's other outcome — a dirtied document — is landed here; only the
    /// relayout has to travel out to whoever holds the `Ui`.
    fn commit(
        &mut self,
        queued: impl IntoIterator<Item = DocumentRequest>,
        library: &Library,
    ) -> EditFacts {
        debug_assert!(self.batch.is_empty(), "the last commit left steps behind");
        let mut signals = StepSignals::default();
        // The gesture every step of the batch so far is part of: it names the
        // batch only when no step came of an edit outside it.
        let mut gesture = BatchGesture::Empty;
        for item in queued {
            let (intent, joins) = match item {
                DocumentRequest::Graph(intent) => {
                    let joins = intent.gesture();
                    (intent, joins)
                }
                DocumentRequest::InGesture { gesture, intent } => {
                    debug_assert!(
                        intent.gesture().is_none_or(|own| own == gesture),
                        "an edit joins one gesture"
                    );
                    (intent, Some(gesture))
                }
                DocumentRequest::Cancel {
                    gesture: cancelled,
                    otherwise,
                } => {
                    // The batch so far is its own entry, so the withdraw finds
                    // the gesture's entry as the frames before left it.
                    self.history.push(&mut self.batch, gesture.named());
                    gesture = BatchGesture::Empty;
                    let withdrawn =
                        self.history
                            .withdraw(cancelled, &mut self.document, &mut |step| {
                                signals.fold(step);
                            });
                    if withdrawn {
                        continue;
                    }
                    (otherwise, Some(cancelled))
                }
                // Applied straight to the layout: pane arrangement is
                // navigation, so it records no step, raises no signal, and
                // breaks no run of graph edits around it.
                DocumentRequest::View(op) => {
                    self.document.layout.apply(op);
                    continue;
                }
            };
            let committed = self.batch.len();
            if let Err(malformed) = intent.commit(&mut self.document, library, &mut self.batch) {
                panic!("a widget built a malformed intent: {malformed}");
            }
            if self.batch.len() == committed {
                continue;
            }
            for step in &self.batch[committed..] {
                signals.fold(step);
            }
            gesture = gesture.and(joins);
        }
        self.history.push(&mut self.batch, gesture.named());
        self.land(&signals)
    }

    /// Replay the last entry backwards, and report whether that stranded the
    /// canvas's cached geometry. With nothing to undo it is a no-op.
    #[must_use]
    pub(crate) fn undo(&mut self) -> EditFacts {
        // Folded into a value first: the replay callback runs while the
        // history is mutably borrowed, so it cannot touch `self`.
        let mut signals = StepSignals::default();
        self.history
            .undo(&mut self.document, &mut |step| signals.fold(step));
        self.land(&signals)
    }

    /// Replay the next entry forwards — the mirror of [`Self::undo`].
    #[must_use]
    pub(crate) fn redo(&mut self) -> EditFacts {
        let mut signals = StepSignals::default();
        self.history
            .redo(&mut self.document, &mut |step| signals.fold(step));
        self.land(&signals)
    }

    /// Land folded [`StepSignals`]. The one place each signal's *effect* is
    /// spelled out, for both the commit path and undo/redo replay, and so the
    /// one place a debug build checks that the edits left the document
    /// valid, as each step promises: a step that forgets part of what it
    /// changes, as a wiring edit that keeps the route of the wire it ended,
    /// fails at the edit rather than at the next save.
    ///
    /// Returns the one signal whose effect is a *call* rather than a stored
    /// flag, so it has to travel back to whoever holds the `Ui`.
    #[must_use]
    fn land(&mut self, signals: &StepSignals) -> EditFacts {
        debug_assert!(
            self.document.validate().is_ok(),
            "the edits left the document invalid: {:?}",
            self.document.validate()
        );
        // A content edit (or an undone/redone one) leaves the document differing
        // from the last save — barring the exact round-trip back to it, where
        // we accept a stale "dirty" rather than tracking saved state
        // precisely.
        self.dirty |= signals.dirtied;
        if signals.facts.node_set {
            self.revision = self.revision.next();
        }
        signals.facts
    }

    /// The state of the graph's node set.
    pub(crate) const fn graph_revision(&self) -> GraphRevision {
        self.revision
    }

    /// Open the document at `path`, made consistent with `library` by
    /// [`Document::reconcile`]. A load that changed something leaves the
    /// document dirty, so a save accepts the changes.
    pub(crate) fn load(path: PathBuf, library: &Library) -> Result<Loaded, DocumentLoadError> {
        let DocumentFile {
            mut document,
            format,
        } = document::load(&path)?;
        let report = document.reconcile(library);
        let open = Self {
            document,
            path: Some(path),
            format,
            dirty: !report.is_empty(),
            ..Self::default()
        };
        Ok(Loaded { open, report })
    }

    /// The document a launching frontend opens: its `argument` when the
    /// command line named a file, otherwise `remembered`, the one its
    /// preferences remember, when they reopen it.
    ///
    /// A named file outranks the remembered one, reopening preference
    /// included — the user asked for that document by name — and a named file
    /// that fails to load degrades to an empty document rather than falling
    /// back to the remembered one, which would read as the file having loaded.
    /// Either way the remembered one stands: a command-line document is this
    /// launch's, and saving it is what makes it the remembered one. A load
    /// that fails is reported to `status`, and a remembered one that fails is
    /// [`Launched::forgot_remembered`].
    pub(crate) fn open_at_launch(
        argument: Option<PathBuf>,
        remembered: Option<&Path>,
        status: &mut StatusLog,
        library: &Library,
    ) -> Launched {
        let (path, is_remembered) = match (argument, remembered) {
            // Made absolute up front: the argument is relative to the shell's
            // working directory, which the file dialogs' anchor and the
            // worker's disk cache both outlive. One that cannot be, an empty
            // one or one under a working directory that is gone, does not
            // load, as memory holds every path absolute.
            (Some(argument), _) => match path::absolute(&argument) {
                Ok(path) => (path, false),
                Err(error) => {
                    status.error(
                        StatusFamily::Document,
                        format!("load failed: {}: {error}", argument.display()),
                    );
                    return Launched {
                        open: Self::default(),
                        forgot_remembered: false,
                    };
                }
            },
            (None, Some(remembered)) => (remembered.to_path_buf(), true),
            (None, None) => {
                return Launched {
                    open: Self::default(),
                    forgot_remembered: false,
                };
            }
        };
        match Self::load_or_report(path, library, status) {
            Some(loaded) => {
                loaded.report.report(status);
                Launched {
                    open: loaded.open,
                    forgot_remembered: false,
                }
            }
            None => Launched {
                open: Self::default(),
                forgot_remembered: is_remembered,
            },
        }
    }

    /// The document at `path`, or `None` with the failure reported to `status`. The changes the
    /// load made are the caller's to report, after anything that would clear the status line.
    pub(crate) fn load_or_report(
        path: PathBuf,
        library: &Library,
        status: &mut StatusLog,
    ) -> Option<Loaded> {
        match Self::load(path, library) {
            Ok(loaded) => Some(loaded),
            Err(error) => {
                status.error(StatusFamily::Document, format!("load failed: {error:#}"));
                None
            }
        }
    }

    /// Write the document to `path` in `format` and adopt both. Clears
    /// [`dirty`](Self::dirty) — only on success, so a failed save leaves the
    /// unsaved work still flagged.
    pub(crate) fn save_to(
        &mut self,
        path: &Path,
        paths: SavedPaths,
        format: DocumentFormat,
    ) -> Result<(), DocumentSaveError> {
        document::save(&self.document, path, paths, format)?;
        self.path = Some(path.to_path_buf());
        self.format = format;
        self.dirty = false;
        Ok(())
    }
}

/// A document a load opened, and what the load changed to open it. A caller
/// reports the changes after any `succeeded(Document)` it raises, which would
/// clear the status line.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub(crate) open: OpenDocument,
    pub(crate) report: LoadReport,
}

/// Which held gesture a batch's steps so far are all part of.
#[derive(Clone, Copy, Debug)]
enum BatchGesture {
    /// No step yet.
    Empty,
    /// Every step so far joins this gesture.
    One(GestureId),
    /// A step joins none, or two join different ones: the batch is a plain entry.
    Mixed,
}

impl BatchGesture {
    /// The batch after an edit whose steps join `gesture`.
    fn and(self, gesture: Option<GestureId>) -> Self {
        match (self, gesture) {
            (Self::Empty, Some(gesture)) => Self::One(gesture),
            (Self::One(open), Some(gesture)) if open == gesture => self,
            _ => Self::Mixed,
        }
    }

    /// The gesture the whole batch is part of, if any.
    const fn named(self) -> Option<GestureId> {
        match self {
            Self::One(gesture) => Some(gesture),
            Self::Empty | Self::Mixed => None,
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::core::document::Document;
    use crate::core::open_document::OpenDocument;

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
