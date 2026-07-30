//! Commit an [`Intent`] or a [`DockOp`] against a [`Document`] (build →
//! no-op filter → write), and forward/backward-replay a stored
//! [`UndoStep`]'s "to"/"from" half. [`commit_intent`],
//! [`commit_dock_op`], [`apply_step`], and [`revert_step`] are the entry
//! points the rest of the crate drives the edit pipeline through. The
//! `build_step` / `apply_step` halves stay public for undo-stack redo,
//! which applies a *stored* step without rebuilding it.

use scenarium::NodeId;

use crate::core::document::Document;
use crate::core::document::dock::DockOp;
use crate::core::edit::intent::build::{build_dock_step, build_step};
use crate::core::edit::intent::types::{
    DockStep, GraphStep, Intent, NodeProperty, Refusal, UndoStep,
};

/// Build, no-op-filter, and apply one `intent` against `target` in a single
/// call — the entry every frontend drives its per-intent loop through. A
/// `SetInput` that retypes wildcard outputs severs nothing: type mismatches
/// are tolerated (the wire draws as mismatched and flattens as unbound —
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
pub(crate) fn commit_intent(intent: Intent, doc: &mut Document) -> Result<UndoStep, Refusal> {
    let step = build_step(intent, doc)?;
    if step.is_noop() {
        return Err(Refusal::Quiet);
    }
    apply_step(&step, doc);
    Ok(step)
}

/// [`commit_intent`] for a dock op: build, no-op-filter, and apply in one call,
/// with no target anywhere in it.
///
/// The result is still an [`UndoStep`] — the undo stack stores one step
/// type — but it is always the `Dock` arm, so the caller records it under
pub(crate) fn commit_dock_op(op: DockOp, doc: &mut Document) -> Result<UndoStep, Refusal> {
    let step = build_dock_step(op, doc)?;
    if step.is_noop() {
        return Err(Refusal::Quiet);
    }
    apply_dock(&step, doc);
    Ok(UndoStep::Dock(step))
}

/// Forward apply: write the step's "to" half to `doc`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
///
/// `doc` is the entry's doc, so it answers for every step in the batch at
/// once: a `Dock` step mutates the layout around the graph, and a `Graph` step
/// the graph itself.
pub(crate) fn apply_step(step: &UndoStep, doc: &mut Document) {
    match step {
        UndoStep::Dock(step) => apply_dock(step, doc),
        UndoStep::Graph(step) => apply_graph(step, doc),
    }
}

/// Forward-apply a dock step.
fn apply_dock(step: &DockStep, doc: &mut Document) {
    doc.layout = step.to.clone();
}

/// Forward-apply a graph-scoped step.
fn apply_graph(step: &GraphStep, doc: &mut Document) {
    match step {
        GraphStep::AddNode {
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
            doc.main_view.item_placements.insert(*node_id, *pos);
        }
        GraphStep::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
            to_selection,
            ..
        } => {
            for (pos, node_id, node) in nodes {
                doc.graph.insert(*node_id, node.clone());
                doc.main_view.item_placements.insert(*node_id, *pos);
            }
            for (port, binding) in bindings {
                doc.graph.set_input_binding(*port, binding.clone());
            }
            for s in subscriptions {
                doc.graph.subscribe(s.emitter, s.event_idx, s.subscriber);
            }
            doc.main_view.selected = to_selection.clone();
        }
        GraphStep::RemoveNode { detached, .. } => {
            let removed = doc.remove_node(&detached.node_id);
            assert_eq!(
                &removed, detached,
                "removal diverged from the recorded step"
            );
        }
        GraphStep::MoveSelection { moves, .. } => {
            for (key, _, to) in moves {
                if let Some(position) = doc.main_view.item_placements.get_mut(key) {
                    *position = *to;
                }
            }
        }
        GraphStep::RenameNode { node_id, to, .. } => {
            doc.graph.find_mut(*node_id).unwrap().name = to.clone();
        }
        GraphStep::SetInput { input, to, .. } => {
            doc.graph.set_input_binding(*input, to.clone());
        }
        GraphStep::SetSelection { to, .. } => {
            doc.main_view.selected = to.clone();
        }
        GraphStep::Raise { key, to_index, .. } => {
            doc.main_view.move_item_to_index(key, *to_index);
        }
        GraphStep::SetNodeProperty { node_id, to, .. } => {
            set_node_property(doc, node_id, *to);
        }
        GraphStep::SetViewport { to, .. } => {
            doc.main_view.viewport = *to;
        }
        GraphStep::SetSubscription {
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
        UndoStep::Dock(step) => revert_dock(step, doc),
        UndoStep::Graph(step) => revert_graph(step, doc),
    }
}

/// Backward-apply a dock step.
fn revert_dock(step: &DockStep, doc: &mut Document) {
    doc.layout = step.from.clone();
}

/// Backward-apply a graph-scoped step.
fn revert_graph(step: &GraphStep, doc: &mut Document) {
    match step {
        GraphStep::AddNode { node_id, .. } => {
            doc.remove_node(node_id);
        }
        GraphStep::DuplicateNodes {
            nodes,
            from_selection,
            ..
        } => {
            // Removing each added node cascade-drops the bindings and
            // subscriptions that referenced it, so the batch's wiring goes
            // with it — only the selection needs explicit restoring.
            for (_, node_id, _) in nodes {
                doc.remove_node(node_id);
            }
            doc.main_view.selected = from_selection.clone();
        }
        GraphStep::RemoveNode {
            detached,
            item_placements,
            selected,
        } => {
            doc.graph.attach_node(detached.clone());
            // Ascending slot order (captured that way), so each insert
            // lands among already-restored earlier slots and the original
            // paint order comes back exactly.
            for (slot, key, position) in item_placements {
                doc.main_view.item_placements.insert(*key, *position);
                doc.main_view.move_item_to_index(key, *slot);
            }
            doc.main_view.selected.extend(selected.iter().copied());
        }
        GraphStep::MoveSelection { moves, .. } => {
            for (key, from, _) in moves {
                if let Some(position) = doc.main_view.item_placements.get_mut(key) {
                    *position = *from;
                }
            }
        }
        GraphStep::RenameNode { node_id, from, .. } => {
            doc.graph.find_mut(*node_id).unwrap().name = from.clone();
        }
        GraphStep::SetInput { input, from, .. } => {
            doc.graph.set_input_binding(*input, from.clone());
        }
        GraphStep::SetSelection { from, .. } => {
            doc.main_view.selected = from.clone();
        }
        GraphStep::Raise {
            key, from_index, ..
        } => {
            doc.main_view.move_item_to_index(key, *from_index);
        }
        GraphStep::SetNodeProperty { node_id, from, .. } => {
            set_node_property(doc, node_id, *from);
        }
        GraphStep::SetViewport { from, .. } => {
            doc.main_view.viewport = *from;
        }
        GraphStep::SetSubscription {
            emitter,
            event_idx,
            subscriber,
            from,
            ..
        } => set_subscription(doc, *emitter, *event_idx, *subscriber, *from),
    }
}
