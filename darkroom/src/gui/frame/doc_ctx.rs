//! [`DocCtx`]: the frame's world and the open document, for the surfaces around the panes.
//!
//! # Decisions
//!
//! - **The window reads the document, a pane reads one graph.** `DocCtx` is what the surround and
//!   the dock take; `GraphCtx` adds one graph, its view, its instance path and its camera, each
//!   resolved once when the context is made, so every accessor stays a field read. Refused: one
//!   context that is both, which fixes "the graph" for the whole window.

use std::collections::BTreeSet;

use scenarium::{GraphId, Library};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::open_document::OpenDocument;
use crate::core::shared_library::SharedLibrary;
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The frame's world and the open document it shows: what the menu bar, the status bar, the tab
/// strip and the dock read, and what each graph pane's
/// [`GraphCtx`](crate::gui::graph_ctx::GraphCtx) narrows to one graph. `Copy`, as every context
/// of the chain is.
///
/// It takes the [`OpenDocument`] rather than the bare [`Document`] because the window shows both
/// halves of it: the graphs and their pane arrangement, and the unsaved-changes flag, a fact about
/// the document *and* the file behind it. Read-only, so nothing here can reach the edit history
/// the pair also holds.
///
/// **Composed per phase, never per frame.** The document reference cannot outlive the phase that
/// took it: the session drains the frame's requests into the document between phases, and that
/// mutation needs the document exclusively.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DocCtx<'a> {
    app: AppCtx<'a>,
    open: &'a OpenDocument,
}

impl<'a> DocCtx<'a> {
    pub(crate) const fn new(app: AppCtx<'a>, open: &'a OpenDocument) -> Self {
        Self { app, open }
    }

    /// The open document whole: its content *and* the file it came from. For the tab strip,
    /// which also shows [`OpenDocument::is_dirty`].
    pub(crate) const fn open(self) -> &'a OpenDocument {
        self.open
    }

    pub(crate) const fn document(self) -> &'a Document {
        self.open.document()
    }

    /// The graph the window's commands act on.
    pub(crate) const fn active(self) -> GraphId {
        self.document().active
    }

    /// The active graph's committed selection: what a menu command and the status bar read.
    pub(crate) fn selected(self) -> &'a BTreeSet<ItemId> {
        &self.document().view(self.active()).selected
    }

    pub(crate) const fn theme(self) -> &'a Theme {
        self.app.theme()
    }

    pub(crate) const fn library(self) -> &'a Library {
        self.app.library()
    }

    pub(crate) const fn run_state(self) -> &'a RunState {
        self.app.run_state()
    }

    /// The library folder's shared subgraphs, as the last scan read them.
    pub(crate) const fn shared(self) -> &'a SharedLibrary {
        self.app.shared()
    }
}
