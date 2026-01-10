use egui::Vec2;
use graph::graph::{Binding, NodeBehavior, NodeId};

use crate::gui::graph_ui::Error;
use crate::model::{IncomingConnection, ViewNode};

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,

    pending_action: Option<GraphUiAction>,
}

#[derive(Debug, Clone)]
pub enum GraphUiAction {
    CacheToggled {
        node_id: NodeId,
        after: NodeBehavior,
    },
    InputChanged {
        node_id: NodeId,
        input_idx: usize,
        before: Binding,
        after: Binding,
    },
    NodeRemoved {
        view_node: ViewNode,
        incoming: Vec<IncomingConnection>,
    },
    NodeMoved {
        node_id: NodeId,
        before: Vec2,
        after: Vec2,
    },
    NodeSelected {
        before: Option<NodeId>,
        after: Option<NodeId>,
    },
    ZoomPanChanged {
        before_pan: Vec2,
        before_scale: f32,
        after_pan: Vec2,
        after_scale: f32,
    },
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.errors.clear();
        self.run = false;
        self.pending_action = None;
    }

    pub fn add_action(&mut self, action: GraphUiAction) {
        if action.immediate() {
            self.flush();
            self.actions.push(action);
        } else {
            self.add_pending_action(action);
        }
    }

    pub fn add_error(&mut self, error: Error) {
        self.errors.push(error);
    }

    fn add_pending_action(&mut self, action: GraphUiAction) {
        if self.pending_action.is_none() {
            self.pending_action = Some(action);
            return;
        }

        let pending = self.pending_action.take().unwrap();
        if std::mem::discriminant(&pending) != std::mem::discriminant(&action) {
            self.actions.push(pending);
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
                self.actions.push(pending);
                self.pending_action = Some(action);
            }
        }
    }

    fn flush(&mut self) {
        if let Some(action) = self.pending_action.take() {
            self.actions.push(action);
        }
    }
}

impl GraphUiAction {
    pub fn affects_computation(&self) -> bool {
        match self {
            GraphUiAction::NodeRemoved { .. }
            | GraphUiAction::InputChanged { .. }
            | GraphUiAction::CacheToggled { .. } => true,

            GraphUiAction::NodeMoved { .. }
            | GraphUiAction::NodeSelected { .. }
            | GraphUiAction::ZoomPanChanged { .. } => false,
        }
    }
    pub fn immediate(&self) -> bool {
        match self {
            GraphUiAction::CacheToggled { .. }
            | GraphUiAction::InputChanged { .. }
            | GraphUiAction::NodeRemoved { .. }
            | GraphUiAction::NodeSelected { .. } => true,
            GraphUiAction::NodeMoved { .. } | GraphUiAction::ZoomPanChanged { .. } => false,
        }
    }
}
