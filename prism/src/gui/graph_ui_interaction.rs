use scenarium::graph::NodeId;

use crate::{gui::graph_ui::Error, model::graph_ui_action::GraphUiAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCommand {
    StartAutorun,
    StopAutorun,
    RunOnce,
}

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    coalesced_actions: Vec<GraphUiAction>,
    immediate_actions: Vec<GraphUiAction>,
    errors: Vec<Error>,
    run_cmd: Option<RunCommand>,
    request_argument_values: Option<NodeId>,

    pending_action: Option<GraphUiAction>,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.coalesced_actions.clear();
        self.immediate_actions.clear();
        self.errors.clear();
        self.run_cmd = None;
        self.request_argument_values = None;
    }

    pub fn action_stacks(&self) -> impl Iterator<Item = &'_ [GraphUiAction]> {
        [
            (!self.coalesced_actions.is_empty()).then_some(self.coalesced_actions.as_slice()),
            (!self.immediate_actions.is_empty()).then_some(self.immediate_actions.as_slice()),
        ]
        .into_iter()
        .flatten()
    }

    pub fn add_action(&mut self, action: GraphUiAction) {
        if action.immediate() {
            self.flush();
            self.immediate_actions.push(action);
        } else {
            self.add_pending_action(action);
        }
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

    fn add_pending_action(&mut self, action: GraphUiAction) {
        assert!(!action.immediate());

        if self.pending_action.is_none() {
            self.pending_action = Some(action);
            return;
        }

        let pending = self.pending_action.take().unwrap();
        assert!(!pending.immediate());
        if std::mem::discriminant(&pending) != std::mem::discriminant(&action) {
            self.coalesced_actions.push(pending);
            self.pending_action = Some(action);
            return;
        }

        match (&pending, &action) {
            (
                GraphUiAction::NodeMoved {
                    node_id: node_id1,
                    before,
                    ..
                },
                GraphUiAction::NodeMoved {
                    node_id: node_id2,
                    after,
                    ..
                },
            ) if node_id1 == node_id2 => {
                self.pending_action = Some(GraphUiAction::NodeMoved {
                    node_id: *node_id1,
                    before: *before,
                    after: *after,
                });
            }
            (
                GraphUiAction::ZoomPanChanged {
                    before_pan,
                    before_scale,
                    ..
                },
                GraphUiAction::ZoomPanChanged {
                    after_pan,
                    after_scale,
                    ..
                },
            ) => {
                self.pending_action = Some(GraphUiAction::ZoomPanChanged {
                    before_pan: *before_pan,
                    before_scale: *before_scale,
                    after_pan: *after_pan,
                    after_scale: *after_scale,
                });
            }
            _ => {
                self.coalesced_actions.push(pending);
                self.pending_action = Some(action);
            }
        }
    }

    pub fn flush(&mut self) {
        if let Some(pending) = self.pending_action.take() {
            self.coalesced_actions.push(pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    /// Regression: `NodeMoved` used to sit forever in `pending_action`
    /// because it was marked non-immediate, so a drag release never
    /// committed to the action stacks until a *subsequent* action
    /// flushed it — the node visually snapped back on release and then
    /// jumped forward on the next interaction. After Step 4.1 drag
    /// fires a single `NodeMoved` on release, so it must be immediate.
    #[test]
    fn node_moved_lands_in_action_stacks_immediately() {
        let mut interaction = GraphUiInteraction::default();
        interaction.add_action(GraphUiAction::NodeMoved {
            node_id: NodeId::unique(),
            before: Pos2::ZERO,
            after: Pos2::new(10.0, 20.0),
        });

        let actions: Vec<_> = interaction.action_stacks().flatten().collect();
        assert_eq!(
            actions.len(),
            1,
            "NodeMoved must be visible in action_stacks() on the frame it is emitted"
        );
        assert!(
            interaction.pending_action.is_none(),
            "NodeMoved is immediate — nothing should linger in pending_action"
        );
    }

    /// `ZoomPanChanged` keeps the cross-frame coalescing behaviour: one
    /// emission on its own sits in `pending_action` so that follow-ups
    /// can merge into a single undoable change.
    #[test]
    fn zoom_pan_changed_stays_pending_for_coalescing() {
        let mut interaction = GraphUiInteraction::default();
        interaction.add_action(GraphUiAction::ZoomPanChanged {
            before_pan: egui::Vec2::ZERO,
            before_scale: 1.0,
            after_pan: egui::Vec2::new(5.0, 5.0),
            after_scale: 1.2,
        });

        assert_eq!(interaction.action_stacks().count(), 0);
        assert!(interaction.pending_action.is_some());
    }
}
