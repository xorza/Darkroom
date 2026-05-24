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
//!   - [`build_step`] — read snapshot from `&Document`, fold with the
//!     incoming intent, return a fully-populated [`UndoStep`]. Pure.
//!   - [`apply_step`] — write the "to" half of an `UndoStep` to
//!     `&mut Document`. Used both during initial commit and during
//!     undo-stack redo.
//!   - [`revert_step`] — write the "from" half of an `UndoStep` to
//!     `&mut Document`. Used during undo.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::graph::{Binding, Node, NodeBehavior, NodeId};
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::model::ViewNode;

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
///   3. add an arm to [`build_step`] (read `from` from `&Document`
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
    /// Replace the whole selection set. The rubber band, node clicks,
    /// and Esc-deselect all funnel through this — the caller computes
    /// the desired final set and the undo layer captures the prior one.
    SetSelection {
        to: BTreeSet<NodeId>,
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

/// Self-contained undo-stack entry. Each variant carries both halves:
/// the forward "to" payload (read by [`apply_step`]) and the backward
/// "from" payload (read by [`revert_step`]). Built from an [`Intent`]
/// via [`build_step`], which captures the pre-mutation state from
/// `&Document` at commit time.
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
    SetSelection {
        from: BTreeSet<NodeId>,
        to: BTreeSet<NodeId>,
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

/// 1e-4 is the threshold below which two pan/scale samples are
/// considered the same gesture — keeps idle pan/zoom from polluting
/// the undo stack with sub-pixel deltas.
const VIEWPORT_EPS: f32 = 1e-4;

impl UndoStep {
    /// True when applying this step would leave the graph unchanged.
    /// Generic from/to equality across every variant that captures
    /// both halves; viewport uses an epsilon to absorb idle jitter.
    /// `AddNode` / `RemoveNode` are never no-ops (the existence flip
    /// is the change). Filtered out post-`build_step` so phantom
    /// entries (re-selecting the same node, dragging zero pixels)
    /// don't pollute the undo stack.
    pub fn is_noop(&self) -> bool {
        match self {
            UndoStep::AddNode { .. } | UndoStep::RemoveNode { .. } => false,
            UndoStep::MoveNode { from, to, .. } => from == to,
            UndoStep::RenameNode { from, to, .. } => from == to,
            UndoStep::SetInput { from, to, .. } => from == to,
            UndoStep::SetSelection { from, to } => from == to,
            UndoStep::SetCacheBehavior { from, to, .. } => from == to,
            UndoStep::SetEventConnection {
                was_present,
                present,
                ..
            } => was_present == present,
            UndoStep::SetViewport {
                from_pan,
                from_scale,
                to_pan,
                to_scale,
            } => {
                (*from_pan - *to_pan).length_squared() < VIEWPORT_EPS * VIEWPORT_EPS
                    && (*from_scale - *to_scale).abs() < VIEWPORT_EPS
            }
        }
    }
}

/// Read pre-mutation state from `doc` and fold it with `intent`
/// into a complete [`UndoStep`]. Pure — does not write to the graph.
/// Returns `None` when the intent targets a node that no longer exists
/// (e.g. a `MoveNode` whose anchor lingered one frame past a `RemoveNode`
/// applied earlier in the same frame). Callers should treat a `None`
/// result as "stale intent, drop it".
pub fn build_step(intent: Intent, doc: &Document) -> Option<UndoStep> {
    Some(match intent {
        Intent::AddNode { view_node, node } => UndoStep::AddNode { view_node, node },
        Intent::RemoveNode { node_id } => {
            let view_node = doc.view_nodes.by_key(&node_id)?.clone();
            let node = doc.graph.by_id(&node_id)?.clone();
            let mut incoming_connections = Vec::new();
            let mut incoming_events = Vec::new();
            for other in doc.graph.iter() {
                for (input_idx, input) in other.inputs.iter().enumerate() {
                    let Binding::Bind(binding) = &input.binding else {
                        continue;
                    };
                    if binding.node_id == node_id {
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
            let was_selected = doc.selected_nodes.contains(&node_id);
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
            from: doc.view_nodes.by_key(&node_id)?.pos,
            to,
        },
        Intent::RenameNode { node_id, to } => UndoStep::RenameNode {
            from: doc.graph.by_id(&node_id)?.name.clone(),
            node_id,
            to,
        },
        Intent::SetInput {
            node_id,
            input_idx,
            to,
        } => {
            let node = doc.graph.by_id(&node_id)?;
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
        Intent::SetSelection { to } => UndoStep::SetSelection {
            from: doc.selected_nodes.clone(),
            to,
        },
        Intent::SetCacheBehavior { node_id, to } => UndoStep::SetCacheBehavior {
            from: doc.graph.by_id(&node_id)?.behavior,
            node_id,
            to,
        },
        Intent::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
        } => {
            let node = doc.graph.by_id(&event_node_id)?;
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
            from_pan: doc.pan,
            from_scale: doc.scale,
            to_pan: pan,
            to_scale: scale,
        },
    })
}

/// Forward apply: write the step's "to" half to `doc`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
pub fn apply_step(step: &UndoStep, doc: &mut Document) {
    match step {
        UndoStep::AddNode { view_node, node } => {
            assert!(
                doc.graph.by_id(&node.id).is_none(),
                "apply AddNode expects node to be absent"
            );
            doc.graph.add(node.clone());
            doc.view_nodes.add(view_node.clone());
        }
        UndoStep::RemoveNode { node, .. } => {
            assert!(
                doc.graph.by_id(&node.id).is_some(),
                "apply RemoveNode expects node to be present"
            );
            doc.remove_node(&node.id);
        }
        UndoStep::MoveNode { node_id, to, .. } => {
            doc.view_nodes.by_key_mut(node_id).unwrap().pos = *to;
        }
        UndoStep::RenameNode { node_id, to, .. } => {
            doc.graph.by_id_mut(node_id).unwrap().name = to.clone();
        }
        UndoStep::SetInput {
            node_id,
            input_idx,
            to,
            ..
        } => {
            let node = doc.graph.by_id_mut(node_id).unwrap();
            assert!(
                *input_idx < node.inputs.len(),
                "apply SetInput: input index out of range"
            );
            node.inputs[*input_idx].binding = to.clone();
        }
        UndoStep::SetSelection { to, .. } => {
            doc.selected_nodes = to.clone();
        }
        UndoStep::SetCacheBehavior { node_id, to, .. } => {
            doc.graph.by_id_mut(node_id).unwrap().behavior = *to;
        }
        UndoStep::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
            ..
        } => {
            let node = doc.graph.by_id_mut(event_node_id).unwrap();
            assert!(
                *event_idx < node.events.len(),
                "apply SetEventConnection: event index out of range"
            );
            let subscribers = &mut node.events[*event_idx].subscribers;
            let position = subscribers.iter().position(|id| id == subscriber);
            // Idempotent: match `revert_step` so apply/redo and undo
            // behave the same when the target state already holds.
            // A genuine logic error (was_present mismatch with reality)
            // surfaces as a no-op here but would have been filtered by
            // `UndoStep::is_noop` upstream when emitted naturally.
            match (present, position) {
                (true, None) => subscribers.push(*subscriber),
                (false, Some(idx)) => {
                    subscribers.remove(idx);
                }
                _ => {}
            }
        }
        UndoStep::SetViewport {
            to_pan, to_scale, ..
        } => {
            doc.pan = *to_pan;
            doc.scale = *to_scale;
        }
    }
}

/// Backward apply: write the step's "from" half to `doc`. Pairs
/// with [`apply_step`]; calling one after the other restores the
/// graph to its pre-commit state.
pub fn revert_step(step: &UndoStep, doc: &mut Document) {
    match step {
        UndoStep::AddNode { node, .. } => {
            doc.remove_node(&node.id);
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
                doc.graph.by_id(&node.id).is_none(),
                "revert RemoveNode expects removed node to be absent"
            );
            doc.graph.add(node.clone());
            doc.view_nodes.add(view_node.clone());
            for connection in incoming_connections {
                let other = doc.graph.by_id_mut(&connection.node_id).unwrap();
                other.inputs[connection.input_idx].binding = connection.binding.clone();
            }
            for event in incoming_events {
                let other = doc.graph.by_id_mut(&event.node_id).unwrap();
                other.events[event.event_idx]
                    .subscribers
                    .push(removed_node_id);
            }
            if *was_selected {
                doc.selected_nodes.insert(removed_node_id);
            }
        }
        UndoStep::MoveNode { node_id, from, .. } => {
            doc.view_nodes.by_key_mut(node_id).unwrap().pos = *from;
        }
        UndoStep::RenameNode { node_id, from, .. } => {
            doc.graph.by_id_mut(node_id).unwrap().name = from.clone();
        }
        UndoStep::SetInput {
            node_id,
            input_idx,
            from,
            ..
        } => {
            doc.graph.by_id_mut(node_id).unwrap().inputs[*input_idx].binding = from.clone();
        }
        UndoStep::SetSelection { from, .. } => {
            doc.selected_nodes = from.clone();
        }
        UndoStep::SetCacheBehavior { node_id, from, .. } => {
            doc.graph.by_id_mut(node_id).unwrap().behavior = *from;
        }
        UndoStep::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            was_present,
            ..
        } => {
            let node = doc.graph.by_id_mut(event_node_id).unwrap();
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
            doc.pan = *from_pan;
            doc.scale = *from_scale;
        }
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
        | UndoStep::RenameNode { .. } => true,
        // Viewport is the inner-canvas `TranslateScale`, applied at
        // paint; children arrange in pre-transform space, so a pan/zoom
        // changes nothing the layout engine reads — no Pass B needed.
        UndoStep::SetViewport { .. } => false,
        // The inline const-value editor is recorded only when the
        // binding is `Const(_)`. Flipping Const presence (None ⇄ Const,
        // Bind ⇄ Const) toggles the editor in the widget tree, so the
        // node remeasures and ports shift — connection curves must
        // re-sample their endpoints. Typing inside an existing `Const`
        // keeps the editor present at its `Fixed` size, so the
        // value-only edit (Const → Const) doesn't need a relayout.
        UndoStep::SetInput { from, to, .. } => {
            matches!(from, Binding::Const(_)) != matches!(to, Binding::Const(_))
        }
        UndoStep::SetSelection { .. }
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
        | UndoStep::SetSelection { .. }
        | UndoStep::SetCacheBehavior { .. }
        | UndoStep::SetEventConnection { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKey {
    Viewport,
    NodeDrag(NodeId),
}
