use scenarium::graph::NodeId;

use crate::{gui::graph_ui::ConnectionError, model::graph_ui_action::GraphUiAction};

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

/// Buffer of what render emitted this frame: actions that will apply
/// to `ViewGraph` at end-of-frame (via `Session::commit_actions`),
/// plus side-channel signals (errors, run command, argument-values
/// request).
///
/// Every action is *immediate* — it lands in `actions` on emission
/// and is applied + recorded at end of frame. Cross-frame coalescing
/// for continuous gestures (zoom, pan) happens at the undo-stack
/// level via [`GraphUiAction::gesture_key`]. That split is deliberate:
/// keeping the action buffer stateless across frames makes it
/// compatible with egui's multi-pass rendering, where the same UI
/// callback can run more than once per logical frame.
#[derive(Debug, Default)]
pub(crate) struct FrameOutput {
    actions: Vec<GraphUiAction>,
    errors: Vec<ConnectionError>,
    run_cmd: Option<RunCommand>,
    editor_cmd: Option<EditorCommand>,
    request_argument_values: Option<NodeId>,
}

impl FrameOutput {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.errors.clear();
        self.run_cmd = None;
        self.editor_cmd = None;
        self.request_argument_values = None;
    }

    pub fn actions(&self) -> &[GraphUiAction] {
        &self.actions
    }

    pub fn add_action(&mut self, action: GraphUiAction) {
        self.actions.push(action);
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
    fn actions_land_immediately() {
        let mut output = FrameOutput::default();
        output.add_action(GraphUiAction::NodeMoved {
            node_id: NodeId::unique(),
            before: Pos2::ZERO,
            after: Pos2::new(10.0, 20.0),
        });
        assert_eq!(output.actions().len(), 1);
    }

    #[test]
    fn clear_empties_action_buffer() {
        let mut output = FrameOutput::default();
        output.add_action(GraphUiAction::ZoomPanChanged {
            before_pan: egui::Vec2::ZERO,
            before_scale: 1.0,
            after_pan: egui::Vec2::new(5.0, 5.0),
            after_scale: 1.2,
        });

        output.clear();
        assert!(output.actions().is_empty());
    }
}
