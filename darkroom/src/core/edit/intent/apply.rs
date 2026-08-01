//! Commit a [`GraphIntent`] against a [`Document`] (build →
//! no-op filter → write), and forward/backward-replay a stored
//! [`UndoStep`]'s "to"/"from" half. [`commit_intent`],
//! [`commit_intent`], [`apply_step`], and [`revert_step`] are the entry
//! points the rest of the crate drives the edit pipeline through. The
//! `build_step` / `apply_step` halves stay public for undo-stack redo,
//! which applies a *stored* step without rebuilding it.

use scenarium::NodeId;

use crate::core::document::{Document, ItemPlacement};
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::types::{GraphIntent, NodeProperty, Refusal, UndoStep};

/// Build, no-op-filter, and apply one `intent` against `target` in a single
/// call — the entry every frontend drives its per-intent loop through. A
/// `SetInput` that retypes wildcard outputs severs nothing: type mismatches
/// are tolerated (the wire draws as mismatched and lowers as unbound —
/// see scenarium's `typed_binding`), so the edit stays a single step.
///
/// Returns the committed [`UndoStep`] (the caller records it and reads its
/// `requires_*` signals), or the [`Refusal`] that stopped it — in which case
/// nothing was written. A stale anchor, a cycle-forming bind, and a no-op
/// all refuse [`Refusal::Quiet`]ly; only a payload that could never have
/// applied carries a reason back (see [`build_step`]).
///
/// `build_step` / `apply_step` stay separate for the undo-stack redo path,
/// which applies a stored step without rebuilding it (a redo replays
/// already-valid history).
pub(crate) fn commit_intent(intent: GraphIntent, doc: &mut Document) -> Result<UndoStep, Refusal> {
    let step = build_step(intent, doc)?;
    if step.is_noop() {
        return Err(Refusal::Quiet);
    }
    apply_step(&step, doc);
    Ok(step)
}

/// Forward apply: write the step's "to" half to `doc`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
///
/// `doc` is the entry's doc, so it answers for every step in the batch at once.
pub(crate) fn apply_step(step: &UndoStep, doc: &mut Document) {
    match step {
        UndoStep::AddNode {
            pos,
            node_id,
            node,
            bindings,
        } => {
            // Freshness is established by `build_step`, for every caller;
            // this only catches a stored step replayed out of order.
            assert!(
                doc.graph.find(*node_id).is_none(),
                "apply AddNode expects node to be absent"
            );
            doc.graph.insert(*node_id, node.clone());
            for (port, binding) in bindings {
                doc.graph.set_input_binding(*port, binding.clone());
            }
            let z = doc.main_view.front_z();
            doc.main_view
                .item_placements
                .insert(*node_id, ItemPlacement { pos: *pos, z });
        }
        UndoStep::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
            to_selection,
            ..
        } => {
            for (pos, node_id, node) in nodes {
                doc.graph.insert(*node_id, node.clone());
                let z = doc.main_view.front_z();
                doc.main_view
                    .item_placements
                    .insert(*node_id, ItemPlacement { pos: *pos, z });
            }
            for (port, binding) in bindings {
                doc.graph.set_input_binding(*port, binding.clone());
            }
            for s in subscriptions {
                doc.graph.subscribe(s.emitter, s.event_idx, s.subscriber);
            }
            doc.main_view.selected = to_selection.clone();
        }
        UndoStep::RemoveNode { detached, .. } => {
            let removed = doc.remove_node(detached.node_id);
            assert_eq!(
                &removed, detached,
                "removal diverged from the recorded step"
            );
        }
        UndoStep::MoveSelection { moves, .. } => {
            for (key, _, to) in moves {
                if let Some(placement) = doc.main_view.item_placements.get_mut(key) {
                    placement.pos = *to;
                }
            }
        }
        UndoStep::RenameNode { node_id, to, .. } => {
            doc.graph.find_mut(*node_id).unwrap().name = to.clone();
        }
        UndoStep::SetInput { input, to, .. } => {
            doc.graph.set_input_binding(*input, to.clone());
        }
        UndoStep::SetSelection { to, .. } => {
            doc.main_view.selected = to.clone();
        }
        UndoStep::Raise { key, to_z, .. } => {
            set_item_z(doc, key, *to_z);
        }
        UndoStep::SetNodeProperty { node_id, to, .. } => {
            set_node_property(doc, node_id, *to);
        }
        UndoStep::SetViewport { to, .. } => {
            doc.main_view.viewport = *to;
        }
        UndoStep::SetSubscription {
            emitter,
            event_idx,
            subscriber,
            to,
            ..
        } => set_subscription(doc, *emitter, *event_idx, *subscriber, *to),
    }
}

/// Apply (`subscribed = true`) or remove (`false`) one event subscription.
/// Shared by `apply_graph` (writes `to`) and `revert_graph` (writes `from`).
fn set_subscription(
    doc: &mut Document,
    emitter: NodeId,
    event_idx: usize,
    subscriber: NodeId,
    subscribed: bool,
) {
    if subscribed {
        doc.graph.subscribe(emitter, event_idx, subscriber);
    } else {
        doc.graph.unsubscribe(emitter, event_idx, subscriber);
    }
}

/// Write one [`NodeProperty`] into its node field. Shared by `apply_graph`
/// (writes `to`) and `revert_graph` (writes `from`).
/// Write one item's paint depth. The whole of what a raise does in either
/// direction — no neighbour moves, which is why apply and revert are the same
/// call with a different value.
fn set_item_z(doc: &mut Document, key: &NodeId, z: u32) {
    if let Some(placement) = doc.main_view.item_placements.get_mut(key) {
        placement.z = z;
    }
}

fn set_node_property(doc: &mut Document, node_id: &NodeId, prop: NodeProperty) {
    let node = doc.graph.find_mut(*node_id).unwrap();
    match prop {
        NodeProperty::Disabled(v) => node.disabled = v,
        NodeProperty::RuntimeCache(v) => node.cache = v,
    }
}

/// Backward apply: write the step's "from" half to `doc`. Pairs
/// with [`apply_step`]; calling one after the other restores the
/// graph to its pre-commit state.
pub(crate) fn revert_step(step: &UndoStep, doc: &mut Document) {
    match step {
        UndoStep::AddNode { node_id, .. } => {
            doc.remove_node(*node_id);
        }
        UndoStep::DuplicateNodes {
            nodes,
            from_selection,
            ..
        } => {
            // Removing each added node cascade-drops the bindings and
            // subscriptions that referenced it, so the batch's wiring goes
            // with it — only the selection needs explicit restoring.
            for (_, node_id, _) in nodes {
                doc.remove_node(*node_id);
            }
            doc.main_view.selected = from_selection.clone();
        }
        UndoStep::RemoveNode {
            detached,
            item_placements,
            selected,
        } => {
            doc.graph.attach_node(detached.clone());
            // Ascending slot order (captured that way), so each insert
            // lands among already-restored earlier slots and the original
            // paint order comes back exactly.
            for (key, placement) in item_placements {
                doc.main_view.item_placements.insert(*key, *placement);
            }
            doc.main_view.selected.extend(selected.iter().copied());
        }
        UndoStep::MoveSelection { moves, .. } => {
            for (key, from, _) in moves {
                if let Some(placement) = doc.main_view.item_placements.get_mut(key) {
                    placement.pos = *from;
                }
            }
        }
        UndoStep::RenameNode { node_id, from, .. } => {
            doc.graph.find_mut(*node_id).unwrap().name = from.clone();
        }
        UndoStep::SetInput { input, from, .. } => {
            doc.graph.set_input_binding(*input, from.clone());
        }
        UndoStep::SetSelection { from, .. } => {
            doc.main_view.selected = from.clone();
        }
        UndoStep::Raise { key, from_z, .. } => {
            set_item_z(doc, key, *from_z);
        }
        UndoStep::SetNodeProperty { node_id, from, .. } => {
            set_node_property(doc, node_id, *from);
        }
        UndoStep::SetViewport { from, .. } => {
            doc.main_view.viewport = *from;
        }
        UndoStep::SetSubscription {
            emitter,
            event_idx,
            subscriber,
            from,
            ..
        } => set_subscription(doc, *emitter, *event_idx, *subscriber, *from),
    }
}
