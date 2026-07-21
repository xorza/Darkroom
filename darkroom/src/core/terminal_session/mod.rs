//! Terminal/headless policy over the shared [`Workspace`].
//!
//! This layer only interprets worker and script events for non-GUI frontends
//! and tracks script-requested shutdown. Document/runtime invariants live in
//! [`Workspace`], while the GUI owns its independent editing and undo policy.

use scenarium::Library;

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::apply::commit_intent_cascading;
use crate::core::edit::intent::types::Intent;
use crate::core::io::preferences::Preferences;
use crate::core::script::{ScriptConfig, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::WorkerEvent;
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
        let preferences = Preferences::load();
        let mut workspace = Workspace::new(script_config, wake, &preferences);
        workspace.normalize_document();
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
                    let library = self.workspace.runtime.library.current.clone();
                    self.workspace.open.normalization_pending |=
                        apply_intents(&mut self.workspace.open.document, intents, &library);
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
        let events: Vec<WorkerEvent> = self.workspace.runtime.drain_worker().collect();
        for event in events {
            match event {
                WorkerEvent::ExecutionFinished(Ok(stats)) => {
                    self.workspace.runtime.status.error = None;
                    self.workspace.runtime.status.info(format!(
                        "run finished: {} node(s), {:.3}s",
                        stats.executed_nodes.len(),
                        stats.elapsed_secs
                    ));
                    for log in &stats.logs {
                        self.workspace
                            .runtime
                            .status
                            .info(format!("  [{:?}] {}", log.level, log.message));
                    }
                }
                WorkerEvent::ExecutionFinished(Err(error)) => self
                    .workspace
                    .runtime
                    .status
                    .error(format!("run failed: {error}")),
                WorkerEvent::NodeProgress(_) | WorkerEvent::PinnedOutputs(_) => {}
            }
        }
    }
}

fn apply_intents(document: &mut Document, intents: Vec<Intent>, library: &Library) -> bool {
    let target = document.active_target().unwrap_or(GraphRef::Main);
    let mut normalization_pending = false;
    for intent in intents {
        for step in commit_intent_cascading(intent, document, target, library) {
            normalization_pending |= step.requires_reconcile();
        }
    }
    normalization_pending
}
