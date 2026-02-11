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
    pub errors: Vec<Error>,
    pub run_cmd: Option<RunCommand>,
    pub request_argument_values: Option<NodeId>,

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
