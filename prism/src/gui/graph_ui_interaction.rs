use scenarium::graph::NodeId;

use crate::{gui::graph_ui::Error, model::graph_ui_action::GraphUiAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCommand {
    StartAutorun,
    StopAutorun,
    RunOnce,
}

/// Buffer of what render emitted this frame: actions that will apply
/// to `ViewGraph` at end-of-frame (via `AppData::handle_actions`),
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
pub(crate) struct GraphUiInteraction {
    actions: Vec<GraphUiAction>,
    errors: Vec<Error>,
    run_cmd: Option<RunCommand>,
    request_argument_values: Option<NodeId>,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.errors.clear();
        self.run_cmd = None;
        self.request_argument_values = None;
    }

    /// Iterates the emitted actions. Returned as an iterator of slices
    /// to stay compatible with the old two-stack API while simplifying
    /// the internals — callers just flatten.
    pub fn action_stacks(&self) -> impl Iterator<Item = &'_ [GraphUiAction]> {
        (!self.actions.is_empty())
            .then_some(self.actions.as_slice())
            .into_iter()
    }

    pub fn add_action(&mut self, action: GraphUiAction) {
        self.actions.push(action);
    }

    pub fn add_error(&mut self, error: Error) {
        self.errors.push(error);
    }

    pub fn pop_error(&mut self) -> Option<Error> {
        self.errors.pop()
    }

    pub fn run_cmd(&self) -> Option<RunCommand> {
        self.run_cmd
    }

    pub fn set_run_cmd(&mut self, cmd: RunCommand) {
        self.run_cmd = Some(cmd);
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
    fn actions_land_immediately_in_action_stacks() {
        let mut interaction = GraphUiInteraction::default();
        interaction.add_action(GraphUiAction::NodeMoved {
            node_id: NodeId::unique(),
            before: Pos2::ZERO,
            after: Pos2::new(10.0, 20.0),
        });

        let actions: Vec<_> = interaction.action_stacks().flatten().collect();
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn clear_empties_action_buffer() {
        let mut interaction = GraphUiInteraction::default();
        interaction.add_action(GraphUiAction::ZoomPanChanged {
            before_pan: egui::Vec2::ZERO,
            before_scale: 1.0,
            after_pan: egui::Vec2::new(5.0, 5.0),
            after_scale: 1.2,
        });

        interaction.clear();
        assert_eq!(interaction.action_stacks().count(), 0);
    }
}
