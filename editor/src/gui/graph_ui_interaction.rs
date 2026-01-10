use graph::graph::NodeId;

use crate::gui::graph_ui::Error;

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,
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
        self.actions.push(action);
    }

    pub fn add_error(&mut self, error: Error) {
        self.errors.push(error);
    }

    // pub fn add_node_selected(&mut self, node_id: Option<NodeId>) {
    //     self.add_action(GraphUiAction::NodeSelected { node_id });
    // }
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
