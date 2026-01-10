use graph::graph::NodeId;

use crate::gui::graph_ui::{Error, GraphUiAction};

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,
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

    pub fn add_node_selected(&mut self, node_id: Option<NodeId>) {
        self.add_action(GraphUiAction::NodeSelected { node_id });
    }
}
