//! Frontend-agnostic editing core for the non-GUI modes (`--tui`,
//! `--headless`). Owns the document plus an [`Engine`] (func lib +
//! evaluation worker + script host), and drains their inbound queues on
//! each [`Session::tick`]. No Palantir, no undo stack, no inspector —
//! scripts mutate the graph, trigger runs, and `print`/`shutdown`; the
//! worker's results and script `print`s land on a small status log the
//! driver can show.
//!
//! The GUI ([`crate::gui::app::App`]) is the parallel shell over the same
//! [`Engine`] with its own per-frame orchestration; `tui`/`headless` share
//! this one. The pieces it reuses — [`Document`], the engine, and the
//! `build_step`/`apply_step` intent machinery — are all GUI-free.

use std::collections::VecDeque;
use std::path::PathBuf;

use scenarium::library::Library;
use scenarium::prelude::Graph as CoreGraph;

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::{Intent, commit_intent_cascading};
use crate::core::engine::Engine;
use crate::core::io::config::AppConfig;
use crate::core::io::persistence;
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;

#[cfg(test)]
mod tests;

/// Cap on the retained status log (lines). Oldest lines drop off the front
/// so a long-running session can't grow it without bound.
const STATUS_LOG_CAP: usize = 200;

#[derive(Debug)]
pub(crate) struct Session {
    document: Document,
    /// Shared runtime services (func lib + worker + script host).
    engine: Engine,
    /// Set when a script edit can change a subgraph's derived interface;
    /// the graph is reconciled before the next run / save.
    needs_reconcile: bool,
    /// Rolling status log (worker summaries, script `print`s, errors),
    /// surfaced by the TUI `status` command. Bounded by [`STATUS_LOG_CAP`].
    status: VecDeque<String>,
    /// Where `save` writes, if a document was opened at startup.
    current_path: Option<PathBuf>,
    /// Flipped by a script `shutdown()`; the driver loop polls
    /// [`Session::should_quit`] after each tick.
    quit: bool,
}

impl Session {
    /// Build the core: assemble the func lib, spin up the worker + script
    /// host (both woken through `wake`), and reopen the last document from
    /// the saved config (or seed an empty graph). The script host is `None`
    /// unless `script_cfg` enabled a listener.
    pub(crate) fn new(script_cfg: &ScriptConfig, wake: Wake) -> Self {
        let engine = Engine::new(script_cfg, wake);

        let config = AppConfig::load();
        let (document, current_path) = match config.document_path.as_deref() {
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
            status: VecDeque::new(),
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

    pub(crate) fn status_lines(&self) -> impl Iterator<Item = &str> {
        self.status.iter().map(String::as_str)
    }

    /// Drain the worker + script inbound queues and act on them: apply
    /// script edits to the document, summarize worker results onto the
    /// status log, and (if a script asked) reconcile + run the graph. No
    /// rendering — the driver decides how to surface [`Self::status_lines`].
    pub(crate) fn tick(&mut self) {
        self.drain_worker();
        // Drain to a Vec so the `&mut self.engine` borrow is released before
        // the loop body touches other `self` fields.
        let inbound = self.engine.drain_script();
        let mut run = false;
        for event in inbound {
            match event {
                ScriptMessage::Print { msg } => self.push_status(format!("script: {msg}")),
                ScriptMessage::Apply(intents) => {
                    let library = self.engine.library.load();
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

    /// Reconcile if needed, then send the whole graph to the worker for one
    /// evaluation. Results arrive on a later [`Self::tick`] as status lines.
    pub(crate) fn run_graph(&mut self) {
        self.reconcile_if_needed();
        self.engine.run_once(self.document.graph.clone());
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
            self.document
                .reconcile_boundaries(&self.engine.library.load());
            self.needs_reconcile = false;
        }
    }

    fn drain_worker(&mut self) {
        // Collect to drop the `self.engine` borrow before `push_status`.
        let events: Vec<WorkerEvent> = self.engine.drain_worker().collect();
        for event in events {
            match event {
                WorkerEvent::ExecutionFinished(Ok(stats)) => {
                    self.push_status(format!(
                        "run finished: {} node(s), {:.3}s",
                        stats.executed_nodes.len(),
                        stats.elapsed_secs
                    ));
                    for log in &stats.logs {
                        self.push_status(format!("  [{:?}] {}", log.level, log.message));
                    }
                }
                WorkerEvent::ExecutionFinished(Err(err)) => {
                    self.push_status(format!("run failed: {err}"));
                }
                // Headless/TUI surfaces only the final summary, not live
                // per-node progress or per-node argument values.
                WorkerEvent::NodeProgress(_) => {}
                WorkerEvent::ArgumentValues { .. } => {}
            }
        }
    }

    fn push_status(&mut self, line: String) {
        tracing::info!(target: "darkroom::session", "{line}");
        if self.status.len() >= STATUS_LOG_CAP {
            self.status.pop_front();
        }
        self.status.push_back(line);
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
