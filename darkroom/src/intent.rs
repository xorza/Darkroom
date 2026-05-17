//! Forward-only descriptions of graph mutations + the self-contained
//! undo entries built from them. Ported from `darkroom-egui` —
//! same design rationale; substitute `glam::Vec2` for `egui::{Pos2,Vec2}`.
//!
//! An [`Intent`] is "what the caller wants the graph to look like
//! after"; it carries no history. To make the change reversible, we
//! pair the intent with a snapshot of the slot it overwrites. Rather
//! than carrying that snapshot in a sibling enum, [`UndoStep`] folds
//! both halves into one variant per kind: every variant has both the
//! "from" payload (for revert) and the "to" payload (for forward
//! apply). Type-level enforcement means an `UndoStep` can never be
//! constructed inconsistently — there's no `(Intent::A, Snapshot::B)`
//! mismatch to worry about at runtime.
//!
//! Three free fns own the per-variant logic:
//!   - [`build_step`] — read snapshot from `&ViewGraph`, fold with the
//!     incoming intent, return a fully-populated [`UndoStep`]. Pure.
//!   - [`apply_step`] — write the "to" half of an `UndoStep` to
//!     `&mut ViewGraph`. Used both during initial commit and during
//!     undo-stack redo.
//!   - [`revert_step`] — write the "from" half of an `UndoStep` to
//!     `&mut ViewGraph`. Used during undo.

use glam::Vec2;
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

/// What the caller wants to change. Forward-only — no `from` fields.
/// Each variant says "set X to Y"; the consumer captures the previous
/// Y at commit time via [`build_step`].
///
/// **Adding a variant** — touch four spots:
///   1. add the variant here on `Intent`,
///   2. add the matching variant on [`UndoStep`] (carrying both the
///      forward "to" payload and the backward "from" payload, or just
///      forward fields for pure-creation intents),
///   3. add an arm to [`build_step`] (read `from` from `&ViewGraph`
///      and combine with the intent's `to` into a complete `UndoStep`),
///   4. add arms to [`apply_step`] and [`revert_step`] reading the
///      forward and backward halves respectively,
///   5. update [`affects_computation`] if the variant re-triggers
///      compute,
///   6. update [`gesture_key`] if the variant coalesces in undo
///      history.
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
        to: Vec2,
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

/// 1e-4 is the threshold below which two pan/scale samples are
/// considered the same gesture continuation — keeps idle pan/zoom
/// from polluting the undo stack with sub-pixel deltas.
const VIEWPORT_EPS: f32 = 1e-4;

impl Intent {
    /// True when this intent would be a no-op against the current
    /// view graph (sub-`VIEWPORT_EPS` viewport delta, etc.). Checked at
    /// the model boundary so external sources can't bypass the
    /// renderer's emit-time gate by pushing micro-deltas every frame.
    pub fn is_noop_against(&self, vg: &ViewGraph) -> bool {
        match self {
            Self::SetViewport { pan, scale } => {
                (vg.pan - *pan).length_squared() < VIEWPORT_EPS * VIEWPORT_EPS
                    && (vg.scale - *scale).abs() < VIEWPORT_EPS
            }
            _ => false,
        }
    }
}

/// Self-contained undo-stack entry. Each variant carries both halves:
/// the forward "to" payload (read by [`apply_step`]) and the backward
/// "from" payload (read by [`revert_step`]). Built from an [`Intent`]
/// via [`build_step`], which captures the pre-mutation state from
/// `&ViewGraph` at commit time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoStep {
    /// Pure creation: the "from" state is "node absent", which is
    /// implicit — undo just removes the node by id.
    AddNode { view_node: ViewNode, node: Node },
    /// Pre-removal state lives entirely on the step: every reference
    /// into the doomed node, so undo can fully restore it.
    RemoveNode {
        view_node: ViewNode,
        node: Node,
        incoming_connections: Vec<IncomingConnection>,
        incoming_events: Vec<IncomingEvent>,
        was_selected: bool,
    },
    MoveNode {
        node_id: NodeId,
        from: Vec2,
        to: Vec2,
    },
    RenameNode {
        node_id: NodeId,
        from: String,
        to: String,
    },
    SetInput {
        node_id: NodeId,
        input_idx: usize,
        from: Binding,
        to: Binding,
    },
    SelectNode {
        from: Option<NodeId>,
        to: Option<NodeId>,
    },
    SetCacheBehavior {
        node_id: NodeId,
        from: NodeBehavior,
        to: NodeBehavior,
    },
    SetEventConnection {
        event_node_id: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        was_present: bool,
        present: bool,
    },
    SetViewport {
        from_pan: Vec2,
        from_scale: f32,
        to_pan: Vec2,
        to_scale: f32,
    },
}

/// Read pre-mutation state from `view_graph` and fold it with `intent`
/// into a complete [`UndoStep`]. Pure — does not write to the graph.
pub fn build_step(intent: Intent, view_graph: &ViewGraph) -> UndoStep {
    match intent {
        Intent::AddNode { view_node, node } => UndoStep::AddNode { view_node, node },
        Intent::RemoveNode { node_id } => {
            let view_node = view_graph
                .view_nodes
                .by_key(&node_id)
                .expect("RemoveNode capture expects a view node")
                .clone();
            let node = view_graph
                .graph
                .by_id(&node_id)
                .expect("RemoveNode capture expects a graph node")
                .clone();
            let mut incoming_connections = Vec::new();
            let mut incoming_events = Vec::new();
            for other in view_graph.graph.iter() {
                for (input_idx, input) in other.inputs.iter().enumerate() {
                    let Binding::Bind(binding) = &input.binding else {
                        continue;
                    };
                    if binding.target_id == node_id {
                        incoming_connections.push(IncomingConnection {
                            node_id: other.id,
                            input_idx,
                            binding: input.binding.clone(),
                        });
                    }
                }
                for (event_idx, event) in other.events.iter().enumerate() {
                    if event.subscribers.contains(&node_id) {
                        incoming_events.push(IncomingEvent {
                            node_id: other.id,
                            event_idx,
                        });
                    }
                }
            }
            let was_selected = view_graph.selected_node_id == Some(node_id);
            UndoStep::RemoveNode {
                view_node,
                node,
                incoming_connections,
                incoming_events,
                was_selected,
            }
        }
        Intent::MoveNode { node_id, to } => UndoStep::MoveNode {
            node_id,
            from: view_graph.view_nodes.by_key(&node_id).unwrap().pos,
            to,
        },
        Intent::RenameNode { node_id, to } => UndoStep::RenameNode {
            from: view_graph.graph.by_id(&node_id).unwrap().name.clone(),
            node_id,
            to,
        },
        Intent::SetInput {
            node_id,
            input_idx,
            to,
        } => {
            let node = view_graph.graph.by_id(&node_id).unwrap();
            assert!(
                input_idx < node.inputs.len(),
                "SetInput capture: input index out of range"
            );
            UndoStep::SetInput {
                from: node.inputs[input_idx].binding.clone(),
                node_id,
                input_idx,
                to,
            }
        }
        Intent::SelectNode { to } => UndoStep::SelectNode {
            from: view_graph.selected_node_id,
            to,
        },
        Intent::SetCacheBehavior { node_id, to } => UndoStep::SetCacheBehavior {
            from: view_graph.graph.by_id(&node_id).unwrap().behavior,
            node_id,
            to,
        },
        Intent::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
        } => {
            let node = view_graph.graph.by_id(&event_node_id).unwrap();
            assert!(
                event_idx < node.events.len(),
                "SetEventConnection capture: event index out of range"
            );
            UndoStep::SetEventConnection {
                was_present: node.events[event_idx].subscribers.contains(&subscriber),
                event_node_id,
                event_idx,
                subscriber,
                present,
            }
        }
        Intent::SetViewport { pan, scale } => UndoStep::SetViewport {
            from_pan: view_graph.pan,
            from_scale: view_graph.scale,
            to_pan: pan,
            to_scale: scale,
        },
    }
}

/// Forward apply: write the step's "to" half to `view_graph`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
pub fn apply_step(step: &UndoStep, view_graph: &mut ViewGraph) {
    match step {
        UndoStep::AddNode { view_node, node } => {
            assert!(
                view_graph.graph.by_id(&node.id).is_none(),
                "apply AddNode expects node to be absent"
            );
            view_graph.graph.add(node.clone());
            view_graph.view_nodes.add(view_node.clone());
        }
        UndoStep::RemoveNode { node, .. } => {
            assert!(
                view_graph.graph.by_id(&node.id).is_some(),
                "apply RemoveNode expects node to be present"
            );
            view_graph.remove_node(&node.id);
        }
        UndoStep::MoveNode { node_id, to, .. } => {
            view_graph.view_nodes.by_key_mut(node_id).unwrap().pos = *to;
        }
        UndoStep::RenameNode { node_id, to, .. } => {
            view_graph.graph.by_id_mut(node_id).unwrap().name = to.clone();
        }
        UndoStep::SetInput {
            node_id,
            input_idx,
            to,
            ..
        } => {
            let node = view_graph.graph.by_id_mut(node_id).unwrap();
            assert!(
                *input_idx < node.inputs.len(),
                "apply SetInput: input index out of range"
            );
            node.inputs[*input_idx].binding = to.clone();
        }
        UndoStep::SelectNode { to, .. } => {
            view_graph.selected_node_id = *to;
        }
        UndoStep::SetCacheBehavior { node_id, to, .. } => {
            view_graph.graph.by_id_mut(node_id).unwrap().behavior = *to;
        }
        UndoStep::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
            ..
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
        UndoStep::SetViewport {
            to_pan, to_scale, ..
        } => {
            view_graph.pan = *to_pan;
            view_graph.scale = *to_scale;
        }
    }
}

/// Backward apply: write the step's "from" half to `view_graph`. Pairs
/// with [`apply_step`]; calling one after the other restores the
/// graph to its pre-commit state.
pub fn revert_step(step: &UndoStep, view_graph: &mut ViewGraph) {
    match step {
        UndoStep::AddNode { node, .. } => {
            view_graph.remove_node(&node.id);
        }
        UndoStep::RemoveNode {
            view_node,
            node,
            incoming_connections,
            incoming_events,
            was_selected,
        } => {
            let removed_node_id = node.id;
            assert!(
                view_graph.graph.by_id(&node.id).is_none(),
                "revert RemoveNode expects removed node to be absent"
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
        UndoStep::MoveNode { node_id, from, .. } => {
            view_graph.view_nodes.by_key_mut(node_id).unwrap().pos = *from;
        }
        UndoStep::RenameNode { node_id, from, .. } => {
            view_graph.graph.by_id_mut(node_id).unwrap().name = from.clone();
        }
        UndoStep::SetInput {
            node_id,
            input_idx,
            from,
            ..
        } => {
            view_graph.graph.by_id_mut(node_id).unwrap().inputs[*input_idx].binding = from.clone();
        }
        UndoStep::SelectNode { from, .. } => {
            view_graph.selected_node_id = *from;
        }
        UndoStep::SetCacheBehavior { node_id, from, .. } => {
            view_graph.graph.by_id_mut(node_id).unwrap().behavior = *from;
        }
        UndoStep::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            was_present,
            ..
        } => {
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
        UndoStep::SetViewport {
            from_pan,
            from_scale,
            ..
        } => {
            view_graph.pan = *from_pan;
            view_graph.scale = *from_scale;
        }
    }
}

/// Whether replaying this step should re-trigger graph computation
/// (autorun / dirty-tracking). UI-only changes (selection, position,
/// name, viewport) return false. Exhaustive on purpose so a new
/// variant must declare its computation effect.
pub fn affects_computation(step: &UndoStep) -> bool {
    match step {
        UndoStep::AddNode { .. }
        | UndoStep::RemoveNode { .. }
        | UndoStep::SetInput { .. }
        | UndoStep::SetCacheBehavior { .. }
        | UndoStep::SetEventConnection { .. } => true,
        UndoStep::MoveNode { .. }
        | UndoStep::RenameNode { .. }
        | UndoStep::SelectNode { .. }
        | UndoStep::SetViewport { .. } => false,
    }
}

/// Whether replaying this step changes anything the layout engine
/// reads (node positions, sizes, label text length, viewport
/// transform). When true, `App::frame` calls `ui.request_relayout()`
/// after applying the batch so the next pass picks up the change.
/// UI-only state with no measure/arrange input (selection, cache
/// behavior, model-only bindings) returns false. Exhaustive on
/// purpose — a new variant must declare its layout effect.
pub fn requires_relayout(step: &UndoStep) -> bool {
    match step {
        UndoStep::AddNode { .. }
        | UndoStep::RemoveNode { .. }
        | UndoStep::MoveNode { .. }
        | UndoStep::RenameNode { .. }
        | UndoStep::SetViewport { .. } => true,
        UndoStep::SetInput { .. }
        | UndoStep::SelectNode { .. }
        | UndoStep::SetCacheBehavior { .. }
        | UndoStep::SetEventConnection { .. } => false,
    }
}

/// Identifies "same continuous gesture" for undo coalescing. The undo
/// stack collapses consecutive steps with the same key into one entry
/// (keeping the *first* "from" payload). Two viewport changes coalesce;
/// two `MoveNode`s of the *same* node coalesce.
pub fn gesture_key(step: &UndoStep) -> Option<GestureKey> {
    match step {
        UndoStep::SetViewport { .. } => Some(GestureKey::Viewport),
        UndoStep::MoveNode { node_id, .. } => Some(GestureKey::NodeDrag(*node_id)),
        UndoStep::AddNode { .. }
        | UndoStep::RemoveNode { .. }
        | UndoStep::RenameNode { .. }
        | UndoStep::SetInput { .. }
        | UndoStep::SelectNode { .. }
        | UndoStep::SetCacheBehavior { .. }
        | UndoStep::SetEventConnection { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKey {
    Viewport,
    NodeDrag(NodeId),
}
