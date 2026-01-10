use egui::{Pos2, Vec2};
use graph::graph::{Binding, Node, NodeBehavior, NodeId};

use crate::common::undo_stack::UndoAction;
use crate::gui::graph_ui::Error;
use crate::model::{IncomingConnection, ViewGraph, ViewNode};

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
        node: Node,
        incoming: Vec<IncomingConnection>,
    },
    NodeMoved {
        node_id: NodeId,
        before: Pos2,
        after: Pos2,
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
        assert!(!action.immediate());

        if self.pending_action.is_none() {
            self.pending_action = Some(action);
            return;
        }

        let pending = self.pending_action.take().unwrap();
        assert!(!pending.immediate());
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
    pub fn apply(&self, view_graph: &mut ViewGraph) {
        match self {
            GraphUiAction::CacheToggled { node_id, after } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                node.behavior = *after;
            }
            GraphUiAction::InputChanged {
                node_id,
                input_idx,
                after,
                ..
            } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                assert!(
                    *input_idx < node.inputs.len(),
                    "input index out of range for InputChanged apply"
                );
                node.inputs[*input_idx].binding = after.clone();
            }
            GraphUiAction::NodeRemoved { node, .. } => {
                view_graph.remove_node(&node.id);
            }
            GraphUiAction::NodeMoved { node_id, after, .. } => {
                let view_node = view_graph.view_nodes.by_key_mut(node_id).unwrap();
                view_node.pos = *after;
            }
            GraphUiAction::NodeSelected { after, .. } => {
                view_graph.selected_node_id = *after;
            }
            GraphUiAction::ZoomPanChanged {
                after_pan,
                after_scale,
                ..
            } => {
                view_graph.pan = *after_pan;
                view_graph.scale = *after_scale;
            }
        }
    }

    pub fn undo(&self, view_graph: &mut ViewGraph) {
        match self {
            GraphUiAction::CacheToggled { node_id, after } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                node.behavior.toggle();
                assert_ne!(node.behavior, *after);
            }
            GraphUiAction::InputChanged {
                node_id,
                input_idx,
                before,
                ..
            } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                assert!(
                    *input_idx < node.inputs.len(),
                    "input index out of range for InputChanged undo"
                );
                node.inputs[*input_idx].binding = before.clone();
            }
            GraphUiAction::NodeRemoved {
                view_node,
                node,
                incoming,
            } => {
                assert!(
                    view_graph.graph.by_id(&node.id).is_none(),
                    "undo expects removed node to be absent"
                );
                view_graph.graph.add(node.clone());
                view_graph.view_nodes.add(view_node.clone());
                for connection in incoming {
                    let node = view_graph.graph.by_id_mut(&connection.node_id).unwrap();
                    assert!(
                        connection.input_idx < node.inputs.len(),
                        "incoming connection input index out of range"
                    );
                    node.inputs[connection.input_idx].binding = connection.binding.clone();
                }
            }
            GraphUiAction::NodeMoved {
                node_id, before, ..
            } => {
                let view_node = view_graph.view_nodes.by_key_mut(node_id).unwrap();
                view_node.pos = *before;
            }
            GraphUiAction::NodeSelected { before, .. } => {
                view_graph.selected_node_id = *before;
            }
            GraphUiAction::ZoomPanChanged {
                before_pan,
                before_scale,
                ..
            } => {
                view_graph.pan = *before_pan;
                view_graph.scale = *before_scale;
            }
        }
    }

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

impl UndoAction<ViewGraph> for GraphUiAction {
    fn apply(&self, value: &mut ViewGraph) {
        GraphUiAction::apply(self, value);
    }

    fn undo(&self, value: &mut ViewGraph) {
        GraphUiAction::undo(self, value);
    }
}
