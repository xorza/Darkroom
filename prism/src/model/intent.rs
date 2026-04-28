//! Forward-only descriptions of graph mutations.
//!
//! An [`Intent`] is "what the caller wants the graph to look like
//! after"; it carries no history. Reversing an intent for undo
//! requires a [`Snapshot`] of the slot the intent will overwrite,
//! captured automatically at apply time. The stored undo entry is
//! [`UndoStep`] = `intent` + `snapshot`.
//!
//! Why split: when emit-sites had to carry `before` fields, scripts
//! using `apply()` were forced to look up old values themselves. Stale
//! `before` values silently corrupted undo. With capture-at-apply,
//! callers state intent only and Session handles the rest.

use egui::{Pos2, Vec2};
use scenarium::graph::{Binding, Node, NodeBehavior, NodeId};
use serde::{Deserialize, Serialize};

use crate::model::{ViewGraph, ViewNode};

/// A connection that pointed *into* a node we're about to remove and
/// must be re-established on undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingConnection {
    pub node_id: NodeId,
    pub input_idx: usize,
    pub binding: Binding,
}

/// An event subscription that targeted a node we're about to remove
/// and must be re-subscribed on undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingEvent {
    pub node_id: NodeId,
    pub event_idx: usize,
}

/// What the caller wants to change. Forward-only — no `before` fields.
/// Each variant says "set X to Y"; Session captures the previous Y at
/// commit time.
///
/// **Adding a variant** — touch all five dispatch sites in this file:
///   1. add the variant here on `Intent`,
///   2. add the matching variant on [`Snapshot`] (carrying the
///      "previous Y" payload, or empty for pure-creation intents),
///   3. add an arm to [`capture`] reading the soon-to-be-overwritten
///      state from `&ViewGraph`,
///   4. add an arm to [`apply`] writing the new state to `&mut ViewGraph`,
///   5. add an arm to [`revert`] (matching `(Snapshot, Intent)` pair),
///   6. update [`affects_computation`] if the intent re-triggers compute,
///   7. update [`gesture_key`] if the intent coalesces in undo history.
///
/// The round-trip test in `action_stack/tests.rs` exercises every
/// variant — adding the variant there too is the safety net for the
/// per-variant arms above.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Intent {
    AddNode {
        view_node: ViewNode,
        node: Node,
    },
    RemoveNode {
        node_id: NodeId,
    },
    MoveNode {
        node_id: NodeId,
        to: Pos2,
    },
    RenameNode {
        node_id: NodeId,
        to: String,
    },
    SetInput {
        node_id: NodeId,
        input_idx: usize,
        to: Binding,
    },
    SelectNode {
        to: Option<NodeId>,
    },
    SetCacheBehavior {
        node_id: NodeId,
        to: NodeBehavior,
    },
    /// Add (`present = true`) or remove (`present = false`)
    /// `subscriber` from the event at `(event_node_id, event_idx)`.
    SetEventConnection {
        event_node_id: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        present: bool,
    },
    SetViewport {
        pan: Vec2,
        scale: f32,
    },
}

/// State captured at apply time so undo can restore the slot the
/// intent overwrote. Variants line up 1:1 with `Intent`. Owned by the
/// undo stack, opaque to emitters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Snapshot {
    /// Pure creation: undo is "remove the node we just added".
    AddNode,
    /// Pre-removal state: every reference into the doomed node, so
    /// undo can fully restore it.
    RemoveNode {
        view_node: ViewNode,
        node: Node,
        incoming_connections: Vec<IncomingConnection>,
        incoming_events: Vec<IncomingEvent>,
        was_selected: bool,
    },
    MoveNode {
        from: Pos2,
    },
    RenameNode {
        from: String,
    },
    SetInput {
        from: Binding,
    },
    SelectNode {
        from: Option<NodeId>,
    },
    SetCacheBehavior {
        from: NodeBehavior,
    },
    SetEventConnection {
        was_present: bool,
    },
    SetViewport {
        pan: Vec2,
        scale: f32,
    },
}

/// One undo-stack entry. `intent` is what the user did; `snapshot` is
/// what was there before. Apply walks intent forward, undo walks
/// snapshot back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoStep {
    pub intent: Intent,
    pub snapshot: Snapshot,
}

/// Capture the current state the intent will overwrite. Read-only on
/// `view_graph`; runs *before* `apply` so the snapshot reflects
/// pre-mutation state. Pure dispatch: variant of intent → variant of
/// snapshot.
pub fn capture(intent: &Intent, view_graph: &ViewGraph) -> Snapshot {
    match intent {
        Intent::AddNode { .. } => Snapshot::AddNode,
        Intent::RemoveNode { node_id } => {
            let view_node = view_graph
                .view_nodes
                .by_key(node_id)
                .expect("RemoveNode capture expects a view node")
                .clone();
            let node = view_graph
                .graph
                .by_id(node_id)
                .expect("RemoveNode capture expects a graph node")
                .clone();
            let mut incoming_connections = Vec::new();
            let mut incoming_events = Vec::new();
            for other in view_graph.graph.iter() {
                for (input_idx, input) in other.inputs.iter().enumerate() {
                    let Binding::Bind(binding) = &input.binding else {
                        continue;
                    };
                    if binding.target_id == *node_id {
                        incoming_connections.push(IncomingConnection {
                            node_id: other.id,
                            input_idx,
                            binding: input.binding.clone(),
                        });
                    }
                }
                for (event_idx, event) in other.events.iter().enumerate() {
                    if event.subscribers.contains(node_id) {
                        incoming_events.push(IncomingEvent {
                            node_id: other.id,
                            event_idx,
                        });
                    }
                }
            }
            let was_selected = view_graph.selected_node_id == Some(*node_id);
            Snapshot::RemoveNode {
                view_node,
                node,
                incoming_connections,
                incoming_events,
                was_selected,
            }
        }
        Intent::MoveNode { node_id, .. } => Snapshot::MoveNode {
            from: view_graph.view_nodes.by_key(node_id).unwrap().pos,
        },
        Intent::RenameNode { node_id, .. } => Snapshot::RenameNode {
            from: view_graph.graph.by_id(node_id).unwrap().name.clone(),
        },
        Intent::SetInput {
            node_id, input_idx, ..
        } => {
            let node = view_graph.graph.by_id(node_id).unwrap();
            assert!(
                *input_idx < node.inputs.len(),
                "SetInput capture: input index out of range"
            );
            Snapshot::SetInput {
                from: node.inputs[*input_idx].binding.clone(),
            }
        }
        Intent::SelectNode { .. } => Snapshot::SelectNode {
            from: view_graph.selected_node_id,
        },
        Intent::SetCacheBehavior { node_id, .. } => Snapshot::SetCacheBehavior {
            from: view_graph.graph.by_id(node_id).unwrap().behavior,
        },
        Intent::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            ..
        } => {
            let node = view_graph.graph.by_id(event_node_id).unwrap();
            assert!(
                *event_idx < node.events.len(),
                "SetEventConnection capture: event index out of range"
            );
            Snapshot::SetEventConnection {
                was_present: node.events[*event_idx].subscribers.contains(subscriber),
            }
        }
        Intent::SetViewport { .. } => Snapshot::SetViewport {
            pan: view_graph.pan,
            scale: view_graph.scale,
        },
    }
}

/// Apply the intent forward. The view graph must be in a state
/// consistent with what `capture` returned (no concurrent mutation
/// between capture and apply).
pub fn apply(intent: &Intent, view_graph: &mut ViewGraph) {
    match intent {
        Intent::AddNode { view_node, node } => {
            assert!(
                view_graph.graph.by_id(&node.id).is_none(),
                "apply AddNode expects node to be absent"
            );
            view_graph.graph.add(node.clone());
            view_graph.view_nodes.add(view_node.clone());
        }
        Intent::RemoveNode { node_id } => {
            assert!(
                view_graph.graph.by_id(node_id).is_some(),
                "apply RemoveNode expects node to be present"
            );
            view_graph.remove_node(node_id);
        }
        Intent::MoveNode { node_id, to } => {
            view_graph.view_nodes.by_key_mut(node_id).unwrap().pos = *to;
        }
        Intent::RenameNode { node_id, to } => {
            view_graph.graph.by_id_mut(node_id).unwrap().name = to.clone();
        }
        Intent::SetInput {
            node_id,
            input_idx,
            to,
        } => {
            let node = view_graph.graph.by_id_mut(node_id).unwrap();
            assert!(
                *input_idx < node.inputs.len(),
                "apply SetInput: input index out of range"
            );
            node.inputs[*input_idx].binding = to.clone();
        }
        Intent::SelectNode { to } => {
            view_graph.selected_node_id = *to;
        }
        Intent::SetCacheBehavior { node_id, to } => {
            view_graph.graph.by_id_mut(node_id).unwrap().behavior = *to;
        }
        Intent::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
        } => {
            let node = view_graph.graph.by_id_mut(event_node_id).unwrap();
            assert!(
                *event_idx < node.events.len(),
                "apply SetEventConnection: event index out of range"
            );
            let subscribers = &mut node.events[*event_idx].subscribers;
            let position = subscribers.iter().position(|id| id == subscriber);
            match (present, position) {
                (true, Some(_)) => {
                    panic!("apply SetEventConnection(present=true): subscriber already present")
                }
                (true, None) => subscribers.push(*subscriber),
                (false, Some(idx)) => {
                    subscribers.remove(idx);
                }
                (false, None) => {
                    panic!("apply SetEventConnection(present=false): subscriber not present")
                }
            }
        }
        Intent::SetViewport { pan, scale } => {
            view_graph.pan = *pan;
            view_graph.scale = *scale;
        }
    }
}

/// Undo: walk the snapshot back. `intent` is consulted only when the
/// snapshot needs an id from it (e.g. AddNode, where the snapshot has
/// no payload but we need the node id to remove).
pub fn revert(snapshot: &Snapshot, intent: &Intent, view_graph: &mut ViewGraph) {
    match (snapshot, intent) {
        (Snapshot::AddNode, Intent::AddNode { node, .. }) => {
            view_graph.remove_node(&node.id);
        }
        (
            Snapshot::RemoveNode {
                view_node,
                node,
                incoming_connections,
                incoming_events,
                was_selected,
            },
            Intent::RemoveNode { .. },
        ) => {
            let removed_node_id = node.id;
            assert!(
                view_graph.graph.by_id(&node.id).is_none(),
                "undo RemoveNode expects removed node to be absent"
            );
            view_graph.graph.add(node.clone());
            view_graph.view_nodes.add(view_node.clone());
            for connection in incoming_connections {
                let other = view_graph.graph.by_id_mut(&connection.node_id).unwrap();
                other.inputs[connection.input_idx].binding = connection.binding.clone();
            }
            for event in incoming_events {
                let other = view_graph.graph.by_id_mut(&event.node_id).unwrap();
                other.events[event.event_idx]
                    .subscribers
                    .push(removed_node_id);
            }
            if *was_selected {
                view_graph.selected_node_id = Some(removed_node_id);
            }
        }
        (Snapshot::MoveNode { from }, Intent::MoveNode { node_id, .. }) => {
            view_graph.view_nodes.by_key_mut(node_id).unwrap().pos = *from;
        }
        (Snapshot::RenameNode { from }, Intent::RenameNode { node_id, .. }) => {
            view_graph.graph.by_id_mut(node_id).unwrap().name = from.clone();
        }
        (
            Snapshot::SetInput { from },
            Intent::SetInput {
                node_id, input_idx, ..
            },
        ) => {
            view_graph.graph.by_id_mut(node_id).unwrap().inputs[*input_idx].binding = from.clone();
        }
        (Snapshot::SelectNode { from }, Intent::SelectNode { .. }) => {
            view_graph.selected_node_id = *from;
        }
        (Snapshot::SetCacheBehavior { from }, Intent::SetCacheBehavior { node_id, .. }) => {
            view_graph.graph.by_id_mut(node_id).unwrap().behavior = *from;
        }
        (
            Snapshot::SetEventConnection { was_present },
            Intent::SetEventConnection {
                event_node_id,
                event_idx,
                subscriber,
                ..
            },
        ) => {
            let node = view_graph.graph.by_id_mut(event_node_id).unwrap();
            let subscribers = &mut node.events[*event_idx].subscribers;
            let position = subscribers.iter().position(|id| id == subscriber);
            match (was_present, position) {
                (true, Some(_)) | (false, None) => {} // already in target state
                (true, None) => subscribers.push(*subscriber),
                (false, Some(idx)) => {
                    subscribers.remove(idx);
                }
            }
        }
        (Snapshot::SetViewport { pan, scale }, Intent::SetViewport { .. }) => {
            view_graph.pan = *pan;
            view_graph.scale = *scale;
        }
        _ => panic!("revert: snapshot/intent variant mismatch"),
    }
}

/// Whether applying this intent should re-trigger graph computation
/// (autorun / dirty-tracking). UI-only changes (selection, position,
/// name, viewport) return false.
pub fn affects_computation(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::AddNode { .. }
            | Intent::RemoveNode { .. }
            | Intent::SetInput { .. }
            | Intent::SetCacheBehavior { .. }
            | Intent::SetEventConnection { .. }
    )
}

/// Identifies "same continuous gesture" for undo coalescing. The undo
/// stack collapses consecutive intents with the same key into one
/// step (keeping the *first* snapshot). Currently only viewport
/// changes coalesce.
pub fn gesture_key(intent: &Intent) -> Option<GestureKey> {
    match intent {
        Intent::SetViewport { .. } => Some(GestureKey::Viewport),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKey {
    Viewport,
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

    #[test]
    #[should_panic(expected = "apply AddNode expects node to be absent")]
    fn add_node_apply_panics_on_duplicate() {
        let mut vg = empty_graph();
        let (node, view_node) = make_node("foo");
        let intent = Intent::AddNode { node, view_node };
        apply(&intent, &mut vg);
        apply(&intent, &mut vg);
    }

    #[test]
    #[should_panic(expected = "apply RemoveNode expects node to be present")]
    fn remove_node_apply_panics_on_missing() {
        let mut vg = empty_graph();
        let (node, view_node) = make_node("foo");
        let node_id = node.id;
        vg.graph.add(node);
        vg.view_nodes.add(view_node);

        let intent = Intent::RemoveNode { node_id };
        apply(&intent, &mut vg);
        apply(&intent, &mut vg);
    }

    #[test]
    #[should_panic(expected = "subscriber already present")]
    fn set_event_connection_present_panics_on_duplicate() {
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
        let subscriber = subscriber_node.id;
        vg.graph.add(subscriber_node);
        vg.view_nodes.add(subscriber_view);

        let intent = Intent::SetEventConnection {
            event_node_id,
            event_idx: 0,
            subscriber,
            present: true,
        };
        apply(&intent, &mut vg);
        apply(&intent, &mut vg);
    }

    #[test]
    #[should_panic(expected = "subscriber not present")]
    fn set_event_connection_absent_panics_on_missing() {
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
        let subscriber = subscriber_node.id;
        vg.graph.add(subscriber_node);
        vg.view_nodes.add(subscriber_view);

        let intent = Intent::SetEventConnection {
            event_node_id,
            event_idx: 0,
            subscriber,
            present: false,
        };
        apply(&intent, &mut vg);
    }
}
