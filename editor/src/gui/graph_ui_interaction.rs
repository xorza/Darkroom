use egui::Vec2;
use graph::graph::{Binding, NodeBehavior, NodeId};

use crate::gui::graph_ui::Error;
use crate::model::{IncomingConnection, ViewNode};

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,

    pending_actions: Option<GraphUiAction>,
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
        self.pending_actions = None;
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
        if let Some(pending) = self.pending_actions.take() {
            if std::mem::discriminant(&pending) == std::mem::discriminant(&action) {
                //
            } else {
                self.actions.push(pending);
            }
        } else {
            self.pending_actions = Some(action);
        }
    }

    fn flush(&mut self) {
        if let Some(action) = self.pending_actions.take() {
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
