//! Terminal/headless policy over the shared [`Workspace`].
//!
//! This layer only interprets worker and script events for non-GUI frontends
//! and tracks script-requested shutdown. Document/runtime invariants live in
//! [`Workspace`], while the GUI owns its independent editing and undo policy.

use scenarium::{WorkerReport, WorkerStatusKind};

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::apply::commit_intent;
use crate::core::edit::intent::types::{Intent, Refusal};
use crate::core::io::preferences::Preferences;
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::workspace::Workspace;

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(crate) struct TerminalSession {
    pub(crate) workspace: Workspace,
    pub(crate) quit: bool,
}

impl TerminalSession {
    pub(crate) fn new(script_config: &ScriptConfig, wake: Wake) -> Self {
        let mut preferences = Preferences::load();
        let workspace = Workspace::new(script_config, wake, &mut preferences);
        Self {
            workspace,
            quit: false,
        }
    }

    pub(crate) fn tick(&mut self) {
        self.drain_worker();
        let inbound = self.workspace.runtime.drain_script();
        let mut run = false;
        for event in inbound {
            match event {
                ScriptMessage::Print { msg } => {
                    self.workspace.runtime.status.info(format!("script: {msg}"))
                }
                ScriptMessage::Apply(intents) => {
                    for reason in apply_intents(&mut self.workspace.open.document, intents) {
                        self.workspace
                            .runtime
                            .status
                            .error(format!("script edit refused: {reason}"));
                    }
                }
                ScriptMessage::RunOnce => run = true,
                ScriptMessage::Shutdown => self.quit = true,
            }
        }
        if run {
            self.workspace.run_once();
        }
    }

    pub(crate) fn save(&mut self) -> bool {
        let Some(path) = self.workspace.open.path.clone() else {
            return false;
        };
        match self.workspace.save_to(&path) {
            Ok(()) => true,
            Err(error) => {
                self.workspace
                    .runtime
                    .status
                    .error(format!("save failed: {error:#}"));
                false
            }
        }
    }

    fn drain_worker(&mut self) {
        let events = self.workspace.runtime.drain_worker();
        for report in events {
            match report {
                WorkerReport::Status(status) => {
                    if let WorkerStatusKind::Completed {
                        elapsed_secs,
                        executed_node_count,
                        ..
                    } = status.kind
                    {
                        self.workspace.runtime.status.error = None;
                        self.workspace.runtime.status.info(format!(
                            "run finished: {executed_node_count} node(s), {elapsed_secs:.3}s"
                        ));
                        for log in &status.logs {
                            self.workspace
                                .runtime
                                .status
                                .info(format!("  [{:?}] {}", log.level, log.message));
                        }
                    }
                }
                WorkerReport::Error(error) => {
                    self.workspace.runtime.status.error(error.to_string())
                }
                WorkerReport::Installed(_) | WorkerReport::Cleared => {}
            }
        }
    }
}

/// Commit each intent against the active target, returning the reason for
/// every one the edit layer refused as malformed. A stale or no-op intent
/// drops silently; a script's payload is decoded straight into an [`Intent`],
/// so one that could never have applied answers back instead.
fn apply_intents(document: &mut Document, intents: Vec<Intent>) -> Vec<String> {
    let target = document.focused_target().unwrap_or(GraphRef::Main);
    intents
        .into_iter()
        .filter_map(|intent| match commit_intent(intent, document, target) {
            Ok(_) | Err(Refusal::Quiet) => None,
            Err(Refusal::Invalid(reason)) => Some(reason),
        })
        .collect()
}
