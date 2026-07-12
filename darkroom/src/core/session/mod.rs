//! Frontend-agnostic editing core for the non-GUI modes (`--tui`,
//! `--headless`). Owns the document plus an [`Engine`] (func lib +
//! evaluation worker + script host), and drains their inbound queues on
//! each [`Session::tick`]. No Aperture, no undo stack, no inspector —
//! scripts mutate the graph, trigger runs, and `print`/`shutdown`; the
//! worker's results and script `print`s land on the engine's shared
//! [`StatusLog`](crate::core::status::StatusLog) the driver can show.
//!
//! The GUI ([`crate::gui::app::App`]) is the parallel shell over the same
//! [`Engine`] with its own per-frame orchestration; `tui`/`headless` share
//! this one. The pieces it reuses — [`Document`], the engine, and the
//! `build_step`/`apply_step` intent machinery — are all GUI-free.

use std::path::PathBuf;

use scenarium::graph::Graph as CoreGraph;
use scenarium::library::Library;

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::{Intent, commit_intent_cascading};
use crate::core::engine::Engine;
use crate::core::io::persistence;
use crate::core::io::preferences::Preferences;
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(crate) struct Session {
    document: Document,
    /// Shared runtime services (func lib + worker + script host + the
    /// [`StatusLog`](crate::core::status::StatusLog) the TUI `status`
    /// command renders).
    pub(crate) engine: Engine,
    /// Set when a script edit can change a subgraph's derived interface;
    /// the graph is reconciled before the next run / save.
    needs_reconcile: bool,
    /// Where `save` writes, if a document was opened at startup.
    current_path: Option<PathBuf>,
    /// Flipped by a script `shutdown()`; the driver loop polls
    /// [`Session::should_quit`] after each tick.
    quit: bool,
}

impl Session {
    /// Build the core: assemble the func lib, spin up the worker + script
    /// host (both woken through `wake`), and reopen the last document from
    /// the saved preferences (or seed an empty graph). The script host is `None`
    /// unless `script_cfg` enabled a listener.
    pub(crate) fn new(script_cfg: &ScriptConfig, wake: Wake) -> Self {
        let mut engine = Engine::new(script_cfg, wake);

        let preferences = Preferences::load();
        let (document, current_path) = match preferences.document_path.as_deref() {
            Some(path) => match persistence::load_document(path) {
                Some(doc) => (doc, Some(path.to_path_buf())),
                None => (empty_document(), None),
            },
            None => (empty_document(), None),
        };

        // Point the worker at the document's project-local cache (memory-only
        // for a never-saved doc). The session's path is fixed after startup, so
        // this one call suffices.
        engine.set_document_cache(current_path.as_deref());

        let mut session = Self {
            document,
            engine,
            needs_reconcile: true,
            current_path,
            quit: false,
        };
        // Canonicalize the freshly loaded / empty doc before anything runs.
        session.reconcile_if_needed();
        session
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.quit
    }

    pub(crate) fn node_count(&self) -> usize {
        self.document.graph.len()
    }

    /// Drain the worker + script inbound queues and act on them: apply
    /// script edits to the document, summarize worker results onto the
    /// engine's status log, and (if a script asked) reconcile + run the
    /// graph. No rendering — the driver decides how to surface
    /// `engine.status.lines()`.
    pub(crate) fn tick(&mut self) {
        self.drain_worker();
        // Drain to a Vec so the `&mut self.engine` borrow is released before
        // the loop body touches other `self` fields.
        let inbound = self.engine.drain_script();
        let mut run = false;
        for event in inbound {
            match event {
                ScriptMessage::Print { msg } => self.engine.status.info(format!("script: {msg}")),
                ScriptMessage::Apply(intents) => {
                    let library = self.engine.library().clone();
                    self.needs_reconcile |= apply_intents(&mut self.document, intents, &library);
                }
                ScriptMessage::RunOnce => run = true,
                ScriptMessage::Shutdown => self.quit = true,
            }
        }
        if run {
            self.run_graph();
        }
    }

    /// Reconcile if needed, then compile + send the graph to the worker for
    /// one evaluation. A compile error lands on the engine's status log
    /// immediately (reported by [`Engine::run_once`]); run results arrive on
    /// a later [`Self::tick`].
    pub(crate) fn run_graph(&mut self) {
        self.reconcile_if_needed();
        self.engine.run_once(&self.document.graph);
    }

    /// Write the document back to the file it was opened from. Returns
    /// `false` when there's no path to save to (a fresh, never-loaded doc).
    pub(crate) fn save(&mut self) -> bool {
        let Some(path) = self.current_path.clone() else {
            return false;
        };
        self.reconcile_if_needed();
        persistence::save_document(&self.document, &path)
    }

    fn reconcile_if_needed(&mut self) {
        if self.needs_reconcile {
            self.document.reconcile_boundaries(self.engine.library());
            self.needs_reconcile = false;
        }
    }

    fn drain_worker(&mut self) {
        // Collect to drop the channel borrow before the status writes below
        // (both live on `self.engine`).
        let events: Vec<WorkerEvent> = self.engine.drain_worker().collect();
        for event in events {
            match event {
                WorkerEvent::ExecutionFinished(Ok(stats)) => {
                    self.engine.status.error = None;
                    self.engine.status.info(format!(
                        "run finished: {} node(s), {:.3}s",
                        stats.executed_nodes.len(),
                        stats.elapsed_secs
                    ));
                    for log in &stats.logs {
                        self.engine
                            .status
                            .info(format!("  [{:?}] {}", log.level, log.message));
                    }
                }
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    self.engine.status.error(format!("run failed: {err}"));
                }
                // Headless/TUI surfaces only the final summary, not live
                // per-node progress.
                WorkerEvent::NodeProgress(_) => {}
            }
        }
    }
}

/// Seed an empty document (root graph, auto-laid-out) — the startup state
/// when no last document is restored.
fn empty_document() -> Document {
    let mut document: Document = CoreGraph::default().into();
    document.main_view.auto_layout_default(&document.graph);
    document
}

/// Apply a batch of script-sourced `intents` to `document`, returning
/// whether any of them can change a subgraph's derived interface (so the
/// caller reconciles before the next run / save). No undo — the non-GUI
/// frontends don't expose it. No-op and stale intents (anchor node already
/// gone) are dropped per-intent; a `SetInput` that retypes a wildcard output
/// cascades into dropping the now-incompatible downstream wires (`library`
/// resolves the types).
fn apply_intents(document: &mut Document, intents: Vec<Intent>, library: &Library) -> bool {
    // Script edits target the active graph. The non-GUI frontends never open
    // a non-graph tab, so this is `Main` in practice; fall back to it anyway.
    let target = document.active_target().unwrap_or(GraphRef::Main);
    let mut needs_reconcile = false;
    for intent in intents {
        for step in commit_intent_cascading(intent, document, target, library) {
            needs_reconcile |= step.requires_reconcile();
        }
    }
    needs_reconcile
}
