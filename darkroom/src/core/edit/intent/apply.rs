//! Commit an [`Intent`] or a [`DocIntent`] against a [`Document`] (build →
//! no-op filter → write), and forward/backward-replay a stored
//! [`UndoStep`]'s "to"/"from" half. [`commit_intent`],
//! [`commit_doc_intent`], [`apply_step`], and [`revert_step`] are the entry
//! points the rest of the crate drives the edit pipeline through. The
//! `build_step` / `apply_step` halves stay public for undo-stack redo,
//! which applies a *stored* step without rebuilding it.

use scenarium::GraphLink;
use scenarium::{FuncInput, FuncOutput};
use scenarium::{NodeId, NodeKind, NodeSearch};

use crate::core::document::{BoundarySide, Document, EditScope, GraphRef};
use crate::core::edit::intent::build::{build_doc_step, build_step};
use crate::core::edit::intent::types::{
    BatchScope, DetachedBoundaryPort, DocIntent, DocStep, GraphStep, Intent, NodeProperty, Refusal,
    UndoStep,
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
pub(crate) fn commit_intent(
    intent: Intent,
    doc: &mut Document,
    target: GraphRef,
) -> Result<UndoStep, Refusal> {
    let step = build_step(intent, doc, target)?;
    if step.is_noop() {
        return Err(Refusal::Quiet);
    }
    apply_step(&step, doc, BatchScope::Graph(target));
    Ok(step)
}

/// [`commit_intent`] for a document-global intent: build, no-op-filter, and
/// apply in one call, with no target anywhere in it.
///
/// The result is still an [`UndoStep`] — the undo stack stores one step
/// type — but it is always the `Doc` arm, so the caller records it under
/// [`BatchScope::Document`].
pub(crate) fn commit_doc_intent(
    intent: DocIntent,
    doc: &mut Document,
) -> Result<UndoStep, Refusal> {
    let step = build_doc_step(intent, doc)?;
    if step.is_noop() {
        return Err(Refusal::Quiet);
    }
    apply_doc(&step, doc);
    Ok(UndoStep::Doc(step))
}

/// Resolve the right graph+view for a scoped step, run `body`, and
/// no-op if the target graph has since disappeared (a graph deleted
/// while its undo entries linger).
fn with_scope(doc: &mut Document, target: GraphRef, body: impl FnOnce(&mut EditScope<'_>)) {
    if let Some(mut scope) = doc.scope_mut(target) {
        body(&mut scope);
    }
}

/// Forward apply: write the step's "to" half to `doc`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
///
/// `scope` is the *entry's* scope, so it answers for every step in the
/// batch at once — a `Doc` step ignores it, and a `Graph` step is only
/// ever recorded in a graph-scoped entry (see [`graph_target`]).
pub(crate) fn apply_step(step: &UndoStep, doc: &mut Document, scope: BatchScope) {
    match step {
        UndoStep::Doc(step) => apply_doc(step, doc),
        UndoStep::Graph(step) => {
            with_scope(doc, graph_target(scope), |scope| apply_graph(step, scope))
        }
    }
}

/// The graph a [`GraphStep`] resolves against. Its entry is graph-scoped by
/// construction — `commit_batch` records a document-global intent as an
/// entry of its own — so a `Document` scope here means a batch was
/// assembled against the wrong one, not that any input was bad.
fn graph_target(scope: BatchScope) -> GraphRef {
    match scope {
        BatchScope::Graph(target) => target,
        BatchScope::Document => panic!("a graph step was recorded in a document-global entry"),
    }
}

/// Forward-apply a document-global step.
fn apply_doc(step: &DocStep, doc: &mut Document) {
    match step {
        DocStep::Dock { to, .. } => doc.layout = to.clone(),
        DocStep::RenameBoundaryPort {
            graph_id,
            side,
            idx,
            from,
            to,
        } => doc.rename_boundary_port(*graph_id, *side, *idx, from, to),
        DocStep::RenameGraph { id, to, .. } => {
            if let Some(graph) = doc.graph.find_graph_mut(*id) {
                graph.interface.name = to.clone();
            }
        }
        DocStep::AddBoundaryPort {
            graph_id,
            side,
            idx,
            name,
            data_type,
        } => {
            if let Some(graph) = doc.graph.find_graph_mut(*graph_id) {
                let definition = &mut graph.interface;
                match side {
                    BoundarySide::Input => definition
                        .inputs
                        .insert(*idx, FuncInput::optional(name.clone(), data_type.clone())),
                    BoundarySide::Output => definition
                        .outputs
                        .insert(*idx, FuncOutput::new(name.clone(), data_type.clone())),
                }
            }
        }
        DocStep::RemoveBoundaryPort { graph_id, detached } => {
            if let Some(parent) = doc.graph.find_graph_parent_mut(*graph_id) {
                match detached {
                    DetachedBoundaryPort::Input(input) => {
                        let removed = parent.detach_graph_input(*graph_id, input.idx);
                        assert_eq!(&removed, input, "removal diverged from the recorded step");
                    }
                    DetachedBoundaryPort::Output(output) => {
                        let removed = parent.detach_graph_output(*graph_id, output.idx);
                        assert_eq!(&removed, output, "removal diverged from the recorded step");
                    }
                }
            }
        }
    }
}

/// Forward-apply a graph-scoped step against its resolved `EditScope`.
fn apply_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode {
            pos,
            node_id,
            node,
            graph,
            bindings,
        } => {
            // Freshness is established by `build_step`, for every caller;
            // this only catches a stored step replayed out of order.
            assert!(
                scope.graph.find(*node_id, NodeSearch::TopLevel).is_none(),
                "apply AddNode expects node to be absent"
            );
            if let Some((graph_id, nested_graph)) = graph {
                scope
                    .graph
                    .insert_graph(*graph_id, nested_graph.clone_verbatim());
            }
            scope.graph.insert(*node_id, node.clone());
            for (port, binding) in bindings {
                scope.graph.set_input_binding(*port, binding.clone());
            }
            scope.view.item_placements.insert(*node_id, *pos);
        }
        GraphStep::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
            to_selection,
            ..
        } => {
            for (pos, node_id, node) in nodes {
                scope.graph.insert(*node_id, node.clone());
                scope.view.item_placements.insert(*node_id, *pos);
            }
            for (port, binding) in bindings {
                scope.graph.set_input_binding(*port, binding.clone());
            }
            for s in subscriptions {
                scope.graph.subscribe(s.emitter, s.event_idx, s.subscriber);
            }
            scope.view.selected = to_selection.clone();
        }
        GraphStep::RemoveNode { detached, .. } => {
            let removed = scope.remove_node(&detached.node_id);
            assert_eq!(
                &removed, detached,
                "removal diverged from the recorded step"
            );
        }
        GraphStep::MoveSelection { moves, .. } => {
            for (key, _, to) in moves {
                if let Some(position) = scope.view.item_placements.get_mut(key) {
                    *position = *to;
                }
            }
        }
        GraphStep::RenameNode { node_id, to, .. } => {
            scope
                .graph
                .find_mut(*node_id, NodeSearch::TopLevel)
                .unwrap()
                .name = to.clone();
        }
        GraphStep::SetInput { input, to, .. } => {
            scope.graph.set_input_binding(*input, to.clone());
        }
        GraphStep::SetSelection { to, .. } => {
            scope.view.selected = to.clone();
        }
        GraphStep::Raise { key, to_index, .. } => {
            scope.view.move_item_to_index(key, *to_index);
        }
        GraphStep::SetNodeProperty { node_id, to, .. } => {
            set_node_property(scope, node_id, *to);
        }
        GraphStep::DetachGraph {
            node_id,
            to_id,
            graph,
            ..
        } => {
            scope.graph.insert_graph(*to_id, graph.clone_verbatim());
            scope
                .graph
                .find_mut(*node_id, NodeSearch::TopLevel)
                .unwrap()
                .kind = NodeKind::Graph(GraphLink::Local(*to_id));
        }
        GraphStep::SetViewport { to, .. } => {
            scope.view.viewport = *to;
        }
        GraphStep::SetSubscription {
            emitter,
            event_idx,
            subscriber,
            to,
            ..
        } => set_subscription(scope, *emitter, *event_idx, *subscriber, *to),
    }
}

/// Apply (`subscribed = true`) or remove (`false`) one event subscription.
/// Shared by `apply_graph` (writes `to`) and `revert_graph` (writes `from`).
fn set_subscription(
    scope: &mut EditScope<'_>,
    emitter: NodeId,
    event_idx: usize,
    subscriber: NodeId,
    subscribed: bool,
) {
    if subscribed {
        scope.graph.subscribe(emitter, event_idx, subscriber);
    } else {
        scope.graph.unsubscribe(emitter, event_idx, subscriber);
    }
}

/// Write one [`NodeProperty`] into its node field. Shared by `apply_graph`
/// (writes `to`) and `revert_graph` (writes `from`).
fn set_node_property(scope: &mut EditScope<'_>, node_id: &NodeId, prop: NodeProperty) {
    let node = scope
        .graph
        .find_mut(*node_id, NodeSearch::TopLevel)
        .unwrap();
    match prop {
        NodeProperty::Disabled(v) => node.disabled = v,
        NodeProperty::RuntimeCache(v) => node.cache = v,
    }
}

/// Backward apply: write the step's "from" half to `doc`. Pairs
/// with [`apply_step`]; calling one after the other restores the
/// graph to its pre-commit state.
pub(crate) fn revert_step(step: &UndoStep, doc: &mut Document, scope: BatchScope) {
    match step {
        UndoStep::Doc(step) => revert_doc(step, doc),
        UndoStep::Graph(step) => {
            with_scope(doc, graph_target(scope), |scope| revert_graph(step, scope))
        }
    }
}

/// Backward-apply a document-global step.
fn revert_doc(step: &DocStep, doc: &mut Document) {
    match step {
        DocStep::Dock { from, .. } => doc.layout = from.clone(),
        DocStep::RenameBoundaryPort {
            graph_id,
            side,
            idx,
            from,
            to,
        } => doc.rename_boundary_port(*graph_id, *side, *idx, to, from),
        DocStep::RenameGraph { id, from, .. } => {
            if let Some(graph) = doc.graph.find_graph_mut(*id) {
                graph.interface.name = from.clone();
            }
        }
        DocStep::AddBoundaryPort {
            graph_id,
            side,
            idx,
            ..
        } => {
            if let Some(graph) = doc.graph.find_graph_mut(*graph_id) {
                let definition = &mut graph.interface;
                match side {
                    BoundarySide::Input => {
                        definition.inputs.remove(*idx);
                    }
                    BoundarySide::Output => {
                        definition.outputs.remove(*idx);
                    }
                }
            }
        }
        DocStep::RemoveBoundaryPort { graph_id, detached } => {
            if let Some(parent) = doc.graph.find_graph_parent_mut(*graph_id) {
                match detached.clone() {
                    DetachedBoundaryPort::Input(input) => {
                        parent.attach_graph_input(*graph_id, input);
                    }
                    DetachedBoundaryPort::Output(output) => {
                        parent.attach_graph_output(*graph_id, output);
                    }
                }
            }
        }
    }
}

/// Backward-apply a graph-scoped step against its resolved `EditScope`.
fn revert_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode { node_id, graph, .. } => {
            scope.remove_node(node_id);
            if let Some((graph_id, _)) = graph {
                scope.graph.graphs.remove(graph_id);
            }
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
                scope.remove_node(node_id);
            }
            scope.view.selected = from_selection.clone();
        }
        GraphStep::RemoveNode {
            detached,
            item_placements,
            selected,
        } => {
            scope.graph.attach_node(detached.clone());
            // Ascending slot order (captured that way), so each insert
            // lands among already-restored earlier slots and the original
            // paint order comes back exactly.
            for (slot, key, position) in item_placements {
                scope.view.item_placements.insert(*key, *position);
                scope.view.move_item_to_index(key, *slot);
            }
            scope.view.selected.extend(selected.iter().copied());
        }
        GraphStep::MoveSelection { moves, .. } => {
            for (key, from, _) in moves {
                if let Some(position) = scope.view.item_placements.get_mut(key) {
                    *position = *from;
                }
            }
        }
        GraphStep::RenameNode { node_id, from, .. } => {
            scope
                .graph
                .find_mut(*node_id, NodeSearch::TopLevel)
                .unwrap()
                .name = from.clone();
        }
        GraphStep::SetInput { input, from, .. } => {
            scope.graph.set_input_binding(*input, from.clone());
        }
        GraphStep::SetSelection { from, .. } => {
            scope.view.selected = from.clone();
        }
        GraphStep::Raise {
            key, from_index, ..
        } => {
            scope.view.move_item_to_index(key, *from_index);
        }
        GraphStep::SetNodeProperty { node_id, from, .. } => {
            set_node_property(scope, node_id, *from);
        }
        GraphStep::DetachGraph {
            node_id,
            from_id,
            to_id,
            ..
        } => {
            scope
                .graph
                .find_mut(*node_id, NodeSearch::TopLevel)
                .unwrap()
                .kind = NodeKind::Graph(GraphLink::Local(*from_id));
            scope.graph.graphs.remove(to_id);
        }
        GraphStep::SetViewport { from, .. } => {
            scope.view.viewport = *from;
        }
        GraphStep::SetSubscription {
            emitter,
            event_idx,
            subscriber,
            from,
            ..
        } => set_subscription(scope, *emitter, *event_idx, *subscriber, *from),
    }
}
