use graph::graph::{Binding, NodeBehavior, NodeId};

use crate::gui::graph_ui::Error;
use crate::model::{IncomingConnection, ViewNode};

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,

    pending_actions: Vec<GraphUiAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    },
    NodeSelected {
        after: Option<NodeId>,
    },
    ZoomPanChanged,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.errors.clear();
        self.run = false;
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
        let action_kind = std::mem::discriminant(&action);
        let has_other_kind = self
            .pending_actions
            .iter()
            .any(|pending| std::mem::discriminant(pending) != action_kind);
        if has_other_kind {
            self.flush();
        }

        self.pending_actions
            .retain(|pending| std::mem::discriminant(pending) != action_kind);
        self.pending_actions.push(action);
    }

    fn flush(&mut self) {
        self.actions.append(&mut self.pending_actions);
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
            | GraphUiAction::ZoomPanChanged => false,
        }
    }
    pub fn immediate(&self) -> bool {
        match self {
            GraphUiAction::CacheToggled { .. }
            | GraphUiAction::InputChanged { .. }
            | GraphUiAction::NodeRemoved { .. }
            | GraphUiAction::NodeSelected { .. } => true,
            GraphUiAction::NodeMoved { .. } | GraphUiAction::ZoomPanChanged => false,
        }
    }
}
