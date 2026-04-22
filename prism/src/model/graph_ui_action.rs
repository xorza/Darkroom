use egui::{Pos2, Vec2};
use scenarium::graph::{Binding, Node, NodeBehavior, NodeId};
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
        was_selected: bool,
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
                // Idempotent: re-applying an already-applied action is a
                // no-op. Needed so commit_actions can apply uniformly
                // without tracking whether the render pass already did so.
                match change {
                    EventSubscriberChange::Added => {
                        if !subscribers.contains(subscriber) {
                            subscribers.push(*subscriber);
                        }
                    }
                    EventSubscriberChange::Removed => {
                        if let Some(index) = subscribers.iter().position(|id| id == subscriber) {
                            subscribers.remove(index);
                        }
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
                // Idempotent — see EventConnectionChanged comment.
                if view_graph.graph.by_id(&node.id).is_none() {
                    view_graph.graph.add(node.clone());
                    view_graph.view_nodes.add(view_node.clone());
                }
            }
            GraphUiAction::NodeRemoved { node, .. } => {
                if view_graph.graph.by_id(&node.id).is_some() {
                    view_graph.remove_node(&node.id);
                }
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
                was_selected,
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
                if *was_selected {
                    view_graph.selected_node_id = Some(removed_node_id);
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

    /// Returns a key that identifies "same semantic gesture" for the
    /// purposes of undo-history coalescing. When two adjacent undo
    /// entries are both a single action with the same `gesture_key`,
    /// the undo stack merges them into a single entry (keeping the
    /// earlier `before` and the later `after`). This is what lets a
    /// multi-frame zoom/pan scroll end up as one undoable step without
    /// any cross-frame state in the action buffer.
    pub fn gesture_key(&self) -> Option<GestureKey> {
        match self {
            GraphUiAction::ZoomPanChanged { .. } => Some(GestureKey::ZoomPan),
            _ => None,
        }
    }
}

/// Discriminant for actions that coalesce with their predecessor in
/// the undo history. Currently only zoom/pan; added as an enum so new
/// variants (e.g. continuous slider drags) can join the scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKey {
    ZoomPan,
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::graph::{Event, Node, NodeBehavior};

    fn empty_graph() -> ViewGraph {
        ViewGraph::default()
    }

    fn make_node(name: &str) -> (Node, ViewNode) {
        let node = Node {
            id: scenarium::graph::NodeId::unique(),
            name: name.into(),
            func_id: scenarium::function::FuncId::unique(),
            behavior: NodeBehavior::AsFunction,
            inputs: Vec::new(),
            events: Vec::new(),
        };
        let view_node = ViewNode {
            id: node.id,
            pos: egui::Pos2::ZERO,
        };
        (node, view_node)
    }

    /// Applying an action twice must be equivalent to applying it once —
    /// this is the contract `commit_actions` relies on to dispatch
    /// `apply` unconditionally regardless of whether the render pass did
    /// the mutation inline.
    #[test]
    fn node_added_apply_is_idempotent() {
        let mut vg = empty_graph();
        let (node, view_node) = make_node("foo");
        let action = GraphUiAction::NodeAdded {
            node: node.clone(),
            view_node,
        };

        action.apply(&mut vg);
        let after_first = vg.graph.nodes.len();
        action.apply(&mut vg);
        assert_eq!(
            after_first,
            vg.graph.nodes.len(),
            "double-apply must not duplicate the node"
        );
    }

    #[test]
    fn node_removed_apply_is_idempotent() {
        let mut vg = empty_graph();
        let (node, view_node) = make_node("foo");
        vg.graph.add(node.clone());
        vg.view_nodes.add(view_node.clone());

        let action = GraphUiAction::NodeRemoved {
            node: node.clone(),
            view_node,
            incoming_connections: Vec::new(),
            incoming_events: Vec::new(),
            was_selected: false,
        };

        action.apply(&mut vg);
        assert!(vg.graph.by_id(&node.id).is_none());
        // Second apply must not panic despite the node already being gone.
        action.apply(&mut vg);
        assert!(vg.graph.by_id(&node.id).is_none());
    }

    #[test]
    fn event_connection_changed_apply_is_idempotent() {
        let mut vg = empty_graph();
        let (mut event_node, event_view) = make_node("emitter");
        event_node.events.push(Event {
            subscribers: Vec::new(),
            name: "tick".into(),
        });
        let event_node_id = event_node.id;
        vg.graph.add(event_node);
        vg.view_nodes.add(event_view);

        let (subscriber_node, subscriber_view) = make_node("subscriber");
        let subscriber_id = subscriber_node.id;
        vg.graph.add(subscriber_node);
        vg.view_nodes.add(subscriber_view);

        let add = GraphUiAction::EventConnectionChanged {
            event_node_id,
            event_idx: 0,
            subscriber: subscriber_id,
            change: EventSubscriberChange::Added,
        };

        add.apply(&mut vg);
        let subs = &vg.graph.by_id(&event_node_id).unwrap().events[0].subscribers;
        assert_eq!(subs, &vec![subscriber_id]);

        // Second apply: still exactly one subscriber — no panic, no dup.
        add.apply(&mut vg);
        let subs = &vg.graph.by_id(&event_node_id).unwrap().events[0].subscribers;
        assert_eq!(subs, &vec![subscriber_id]);

        let remove = GraphUiAction::EventConnectionChanged {
            event_node_id,
            event_idx: 0,
            subscriber: subscriber_id,
            change: EventSubscriberChange::Removed,
        };
        remove.apply(&mut vg);
        // Double-remove: no panic.
        remove.apply(&mut vg);
        let subs = &vg.graph.by_id(&event_node_id).unwrap().events[0].subscribers;
        assert!(subs.is_empty());
    }
}
