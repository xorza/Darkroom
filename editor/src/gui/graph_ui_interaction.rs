use graph::graph::NodeId;

use crate::gui::graph_ui::Error;

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,
    pending_actions: Vec<GraphUiAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphUiAction {
    CacheToggled { node_id: NodeId },
    InputChanged { node_id: NodeId, input_idx: usize },
    NodeRemoved { node_id: NodeId },
    NodeMoved { node_id: NodeId },
    NodeSelected { node_id: Option<NodeId> },
    ZoomPanChanged,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.errors.clear();
        self.run = false;
    }

    pub fn add_action(&mut self, action: GraphUiAction) {
        match &action {
            GraphUiAction::CacheToggled { .. }
            | GraphUiAction::InputChanged { .. }
            | GraphUiAction::NodeRemoved { .. } => {
                self.flush();
                self.actions.push(action);
            }
            GraphUiAction::NodeMoved { node_id } => {
                self.add_pending_action(GraphUiAction::NodeMoved { node_id: *node_id });
            }
            GraphUiAction::NodeSelected { node_id } => {
                self.add_pending_action(GraphUiAction::NodeSelected { node_id: *node_id });
            }
            GraphUiAction::ZoomPanChanged => {
                self.add_pending_action(GraphUiAction::ZoomPanChanged);
            }
        }
    }

    pub fn add_error(&mut self, error: Error) {
        self.errors.push(error);
    }

    fn add_pending_action(&mut self, action: GraphUiAction) {
        self.pending_actions.push(action);
    }

    fn flush(&mut self) {
        self.actions.extend(self.pending_actions.drain(..));
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
}
