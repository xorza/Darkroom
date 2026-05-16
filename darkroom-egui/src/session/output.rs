use scenarium::graph::NodeId;

use crate::{gui::graph_ui::ConnectionError, model::Intent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCommand {
    StartAutorun,
    StopAutorun,
    RunOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCommand {
    Undo,
    Redo,
}

/// Application-level intents emitted by menu items or keyboard
/// shortcuts. Routed as a MainWindow-local `Option<AppCommand>` (not
/// through `FrameOutput`) so the renderer can't reach
/// `AppCommand::Exit` from inside a graph widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    New,
    Save,
    SaveAs,
    Open,
    OpenSettings,
    Exit,
}

/// Buffer of what render emitted this frame: actions that will apply
/// to `ViewGraph` at end-of-frame (via `Session::commit_actions`),
/// plus side-channel signals consumed by `Session::handle_output`
/// (errors, run command, editor undo/redo, argument-values request).
///
/// Every intent is *immediate* — it lands in `intents` on emission
/// and is applied + recorded at end of frame. Cross-frame coalescing
/// for continuous gestures (zoom, pan) happens at the undo-stack
/// level via [`Intent::gesture_key`]. That split is deliberate:
/// keeping the intent buffer stateless across frames makes it
/// compatible with egui's multi-pass rendering, where the same UI
/// callback can run more than once per logical frame.
///
/// `AppCommand` is intentionally *not* a field here — see [`AppCommand`].
#[derive(Debug, Default)]
pub(crate) struct FrameOutput {
    intents: Vec<Intent>,
    errors: Vec<ConnectionError>,
    run_cmd: Option<RunCommand>,
    editor_cmd: Option<EditorCommand>,
    request_argument_values: Option<NodeId>,
}

impl FrameOutput {
    pub fn clear(&mut self) {
        self.intents.clear();
        self.errors.clear();
        self.run_cmd = None;
        self.editor_cmd = None;
        self.request_argument_values = None;
    }

    /// Borrow the per-frame intent buffer for inspection. Tests use
    /// this to assert what got emitted; production reads via
    /// [`Self::take_intents`].
    #[cfg(test)]
    pub fn intents(&self) -> &[Intent] {
        &self.intents
    }

    pub fn add_intent(&mut self, intent: Intent) {
        self.intents.push(intent);
    }

    /// Drain the per-frame intent buffer, transferring ownership.
    /// `Session::commit_actions` calls this so it can move each intent
    /// into its `UndoStep` without cloning — `AddNode` carries a full
    /// `Node`, so the saved clone matters for batched spawns.
    pub fn take_intents(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.intents)
    }

    pub fn add_error(&mut self, error: ConnectionError) {
        self.errors.push(error);
    }

    pub fn pop_error(&mut self) -> Option<ConnectionError> {
        self.errors.pop()
    }

    pub fn run_cmd(&self) -> Option<RunCommand> {
        self.run_cmd
    }

    pub fn set_run_cmd(&mut self, cmd: RunCommand) {
        self.run_cmd = Some(cmd);
    }

    pub fn editor_cmd(&self) -> Option<EditorCommand> {
        self.editor_cmd
    }

    pub fn set_editor_cmd(&mut self, cmd: EditorCommand) {
        self.editor_cmd = Some(cmd);
    }

    pub fn request_argument_values(&self) -> Option<NodeId> {
        self.request_argument_values
    }

    pub fn set_request_argument_values(&mut self, node_id: NodeId) {
        self.request_argument_values = Some(node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    #[test]
    fn intents_land_immediately() {
        let mut output = FrameOutput::default();
        output.add_intent(Intent::MoveNode {
            node_id: NodeId::unique(),
            to: Pos2::new(10.0, 20.0),
        });
        assert_eq!(output.intents().len(), 1);
    }

    #[test]
    fn clear_empties_intent_buffer() {
        let mut output = FrameOutput::default();
        output.add_intent(Intent::SetViewport {
            pan: egui::Vec2::new(5.0, 5.0),
            scale: 1.2,
        });

        output.clear();
        assert!(output.intents().is_empty());
    }
}
