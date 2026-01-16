use egui::{Pos2, Vec2};
use graph::graph::{Binding, Node, NodeBehavior, NodeId};
use serde::{Deserialize, Serialize};

use crate::model::view_graph::IncomingEvent;
use crate::model::{IncomingConnection, ViewGraph, ViewNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventSubscriberChange {
    Added,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    NodeAdded {
        view_node: ViewNode,
        node: Node,
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
    NodeNameChanged {
        node_id: NodeId,
        before: String,
        after: String,
    },
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
            GraphUiAction::NodeAdded { view_node, node } => {
                view_graph.graph.add(node.clone());
                view_graph.view_nodes.add(view_node.clone());
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
            GraphUiAction::NodeNameChanged { node_id, after, .. } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                node.name = after.clone();
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
            GraphUiAction::NodeAdded { node, .. } => {
                view_graph.remove_node(&node.id);
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
            GraphUiAction::NodeNameChanged {
                node_id, before, ..
            } => {
                let node = view_graph.graph.by_id_mut(node_id).unwrap();
                node.name = before.clone();
            }
        }
    }

    pub fn affects_computation(&self) -> bool {
        match self {
            GraphUiAction::NodeAdded { .. }
            | GraphUiAction::NodeRemoved { .. }
            | GraphUiAction::InputChanged { .. }
            | GraphUiAction::CacheToggled { .. }
            | GraphUiAction::EventConnectionChanged { .. } => true,

            GraphUiAction::NodeMoved { .. }
            | GraphUiAction::NodeSelected { .. }
            | GraphUiAction::ZoomPanChanged { .. }
            | GraphUiAction::NodeNameChanged { .. } => false,
        }
    }

    pub fn immediate(&self) -> bool {
        match self {
            GraphUiAction::CacheToggled { .. }
            | GraphUiAction::InputChanged { .. }
            | GraphUiAction::NodeAdded { .. }
            | GraphUiAction::NodeRemoved { .. }
            | GraphUiAction::EventConnectionChanged { .. }
            | GraphUiAction::NodeSelected { .. }
            | GraphUiAction::NodeNameChanged { .. } => true,
            GraphUiAction::NodeMoved { .. } | GraphUiAction::ZoomPanChanged { .. } => false,
        }
    }
}
