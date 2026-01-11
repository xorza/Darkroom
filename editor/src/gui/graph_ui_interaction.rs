use egui::{Pos2, SliderOrientation, Vec2};
use graph::graph::{Binding, Node, NodeBehavior, NodeId};

use crate::gui::graph_ui::Error;
use crate::model::graph_view::IncomingEvent;
use crate::model::{IncomingConnection, ViewGraph, ViewNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSubscriberChange {
    Added,
    Removed,
}

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    actions1: Vec<GraphUiAction>,
    actions2: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,

    pending_action: Option<GraphUiAction>,
}

#[derive(Debug, Clone)]
pub enum GraphUiAction {
    CacheToggled {
        node_id: NodeId,
        before: NodeBehavior,
        after: NodeBehavior,
    },
    EventConnectionChanged {
        event_node_id: NodeId,
        event_idx: usize,

        subscriber: NodeId,
        change: EventSubscriberChange,
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
        incoming_connections: Vec<IncomingConnection>,
        incoming_events: Vec<IncomingEvent>,
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
        self.clear_actions();
        self.errors.clear();
        self.run = false;
    }

    pub fn actions_stacks(&self) -> impl Iterator<Item = &'_ [GraphUiAction]> {
        [
            (!self.actions1.is_empty()).then_some(self.actions1.as_slice()),
            (!self.actions2.is_empty()).then_some(self.actions2.as_slice()),
        ]
        .into_iter()
        .flatten()
    }

    pub fn clear_actions(&mut self) {
        self.actions1.clear();
        self.actions2.clear();
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

impl GraphUiAction {
    pub fn apply(&self, view_graph: &mut ViewGraph) {
        match self {
            GraphUiAction::CacheToggled { node_id, after, .. } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                node.behavior = *after;
            }
            GraphUiAction::EventConnectionChanged {
                event_node_id,
                event_idx,
                subscriber,
                change,
                ..
            } => {
                let node = view_graph.graph.by_id_mut(event_node_id).unwrap();
                assert!(
                    *event_idx < node.events.len(),
                    "event index out of range for EventConnectionChanged apply"
                );
                let subscribers = &mut node.events[*event_idx].subscribers;
                match change {
                    EventSubscriberChange::Added => {
                        assert!(
                            !subscribers.contains(subscriber),
                            "event subscriber already present on add"
                        );
                        subscribers.push(*subscriber);
                    }
                    EventSubscriberChange::Removed => {
                        let index = subscribers
                            .iter()
                            .position(|node_id| node_id == subscriber)
                            .expect("event subscriber missing on remove");
                        subscribers.remove(index);
                    }
                }
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
            GraphUiAction::CacheToggled {
                node_id, before, ..
            } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                node.behavior = *before;
            }
            GraphUiAction::EventConnectionChanged {
                event_node_id,
                event_idx,
                subscriber,
                change,
                ..
            } => {
                let node = view_graph.graph.by_id_mut(event_node_id).unwrap();
                assert!(
                    *event_idx < node.events.len(),
                    "event index out of range for EventConnectionChanged undo"
                );
                let subscribers = &mut node.events[*event_idx].subscribers;
                match change {
                    EventSubscriberChange::Added => {
                        let index = subscribers
                            .iter()
                            .position(|node_id| node_id == subscriber)
                            .expect("event subscriber missing on undo add");
                        subscribers.remove(index);
                    }
                    EventSubscriberChange::Removed => {
                        assert!(
                            !subscribers.contains(subscriber),
                            "event subscriber already present on undo remove"
                        );
                        subscribers.push(*subscriber);
                    }
                }
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
                incoming_connections,
                incoming_events,
            } => {
                let removed_node_id = node.id;
                assert!(
                    view_graph.graph.by_id(&node.id).is_none(),
                    "undo expects removed node to be absent"
                );
                view_graph.graph.add(node.clone());
                view_graph.view_nodes.add(view_node.clone());
                for connection in incoming_connections {
                    let node = view_graph.graph.by_id_mut(&connection.node_id).unwrap();

                    node.inputs[connection.input_idx].binding = connection.binding.clone();
                }
                for event in incoming_events {
                    let node = view_graph.graph.by_id_mut(&event.node_id).unwrap();

                    let subscribers = &mut node.events[event.event_idx].subscribers;
                    subscribers.push(removed_node_id);
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
            | GraphUiAction::CacheToggled { .. }
            | GraphUiAction::EventConnectionChanged { .. } => true,

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
            | GraphUiAction::EventConnectionChanged { .. }
            | GraphUiAction::NodeSelected { .. } => true,
            GraphUiAction::NodeMoved { .. } | GraphUiAction::ZoomPanChanged { .. } => false,
        }
    }
}
