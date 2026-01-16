use graph::graph::NodeId;

use crate::gui::graph_ui::Error;
use crate::model::GraphUiAction;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RunCommand {
    #[default]
    None,
    StartAutorun,
    StopAutorun,
    RunOnce,
}

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    actions1: Vec<GraphUiAction>,
    actions2: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run_cmd: RunCommand,
    pub request_argument_values: Option<NodeId>,

    pending_action: Option<GraphUiAction>,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions1.clear();
        self.actions2.clear();
        self.errors.clear();
        self.run_cmd = RunCommand::None;
        self.request_argument_values = None;
    }

    pub fn action_stacks(&self) -> impl Iterator<Item = &'_ [GraphUiAction]> {
        [
            (!self.actions1.is_empty()).then_some(self.actions1.as_slice()),
            (!self.actions2.is_empty()).then_some(self.actions2.as_slice()),
        ]
        .into_iter()
        .flatten()
    }

    pub fn add_action(&mut self, action: GraphUiAction) {
        if action.immediate() {
            self.flush();
            self.actions2.push(action);
        } else {
            self.add_pending_action(action);
        }
    }

    pub fn add_error(&mut self, error: Error) {
        self.errors.push(error);
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
            self.actions1.push(pending);
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
                self.actions1.push(pending);
                self.pending_action = Some(action);
            }
        }
    }

    pub fn flush(&mut self) {
        if let Some(pending) = self.pending_action.take() {
            self.actions1.push(pending);
        }
    }
}
