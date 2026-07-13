//! Commit an [`Intent`] against a [`Document`] (build → no-op filter →
//! write), and forward/backward-replay a stored [`UndoStep`]'s "to"/"from"
//! half. [`commit_intent_cascading`], [`apply_step`], and [`revert_step`]
//! are the entry points the rest of the crate drives the edit pipeline
//! through. The `build_step` / `apply_step` halves stay public for
//! undo-stack redo, which applies a *stored* step without rebuilding it.

use scenarium::graph::subgraph::SubgraphRef;
use scenarium::graph::{Binding, NodeId, NodeKind, NodeSearch, OutputPort};
use scenarium::library::Library;

use crate::core::document::{Document, EditScope, GraphRef, SelectionKey};
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::types::{DocStep, GraphStep, Intent, NodeProperty, UndoStep};

/// Build, no-op-filter, and apply one `intent` against `target` in a single
/// call. The per-intent core of [`commit_intent_cascading`] (the entry the
/// frontends use); kept separate so the cascade can drive its own sever
/// intents through the same path.
///
/// Returns the committed [`UndoStep`] (the caller records it and reads its
/// `requires_*` signals), or `None` when the intent was stale (anchor node
/// gone), a no-op, or a bind that would close a data cycle — in all cases
/// nothing was written. `build_step` / `apply_step` stay separate for the
/// undo-stack redo path, which applies a stored step without rebuilding it
/// (a redo replays already-valid history, so it needs no cycle check).
pub(crate) fn commit_intent(
    intent: Intent,
    doc: &mut Document,
    target: GraphRef,
) -> Option<UndoStep> {
    // Reject a bind that would close a data cycle: the planner rejects a cyclic
    // graph outright (`Error::CycleDetected`), so the edit must never land. The
    // GUI snap filter normally stops this earlier; this is the authoritative
    // guard covering every binding path, including any that bypass the canvas.
    if let Intent::SetInput {
        input,
        to: Binding::Bind(src),
        ..
    } = &intent
        && doc
            .graph_for(target)
            .is_some_and(|g| g.would_create_cycle(src.node_id, input.node_id))
    {
        return None;
    }
    let step = build_step(intent, doc, target)?;
    if step.is_noop() {
        return None;
    }
    apply_step(&step, doc, target);
    Some(step)
}

/// [`commit_intent`], plus the cascaded edits an input change implies: when a
/// `SetInput` retypes a node's *wildcard* output (a passthrough / reroute), every
/// downstream wire that no longer typechecks is dropped — in the same batch, so
/// undo restores the binding and the severed edges together. `library` resolves
/// the port types. Returns every committed step (the triggering one first), so
/// the caller records / inspects them as one unit. Both the GUI editor and the
/// headless session drive their forward-apply loop through this.
pub(crate) fn commit_intent_cascading(
    intent: Intent,
    doc: &mut Document,
    target: GraphRef,
    library: &Library,
) -> Vec<UndoStep> {
    // Only a `SetInput` can retype a node's output, so only it can invalidate
    // downstream wires. Capture which input changed before the intent is moved.
    let retyped = match &intent {
        Intent::SetInput { input, .. } => Some(*input),
        _ => None,
    };
    let Some(step) = commit_intent(intent, doc, target) else {
        return Vec::new();
    };
    let mut steps = vec![step];
    if let Some(input) = retyped {
        // The engine resolves which wires the retype invalidated (transitively,
        // through any chain of wildcard outputs); drop each in the same batch.
        let severed = doc
            .graph_for(target)
            .map(|graph| graph.edges_invalidated_by(library, input))
            .unwrap_or_default();
        for dst in severed {
            steps.extend(commit_intent(
                Intent::SetInput {
                    input: dst,
                    to: Binding::None,
                },
                doc,
                target,
            ));
        }
    }
    steps
}

/// Resolve the right graph+view for a scoped step, run `body`, and
/// no-op if the target graph has since disappeared (a subgraph deleted
/// while its undo entries linger).
fn with_scope(doc: &mut Document, target: GraphRef, body: impl FnOnce(&mut EditScope<'_>)) {
    if let Some(mut scope) = doc.scope_mut(target) {
        body(&mut scope);
    }
}

/// Forward apply: write the step's "to" half to `doc`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
pub(crate) fn apply_step(step: &UndoStep, doc: &mut Document, target: GraphRef) {
    match step {
        UndoStep::Doc(step) => apply_doc(step, doc),
        UndoStep::Graph(step) => with_scope(doc, target, |scope| apply_graph(step, scope)),
    }
}

/// Forward-apply a document-global step.
fn apply_doc(step: &DocStep, doc: &mut Document) {
    match step {
        DocStep::Dock { to, .. } => doc.layout = to.clone(),
        DocStep::RenameBoundaryPort {
            sub_id,
            side,
            idx,
            from,
            to,
        } => doc.rename_boundary_port(*sub_id, *side, *idx, from, to),
        DocStep::RenameSubgraph { id, to, .. } => {
            if let Some(def) = doc.graph.subgraphs.by_key_mut(id) {
                def.name = to.clone();
            }
        }
    }
}

/// Forward-apply a graph-scoped step against its resolved `EditScope`.
fn apply_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode {
            view_node,
            node,
            def,
            bindings,
        } => {
            assert!(
                scope
                    .graph
                    .find_node(&node.id, NodeSearch::TopLevel)
                    .is_none(),
                "apply AddNode expects node to be absent"
            );
            if let Some(def) = def {
                scope.graph.subgraphs.add((**def).clone());
            }
            scope.graph.add(node.clone());
            for (port, binding) in bindings {
                scope.graph.set_input_binding(*port, binding.clone());
            }
            scope.view.view_nodes.add(view_node.clone());
        }
        GraphStep::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
            to_selection,
            ..
        } => {
            for (view_node, node) in nodes {
                scope.graph.add(node.clone());
                scope.view.view_nodes.add(view_node.clone());
            }
            for (port, binding) in bindings {
                scope.graph.set_input_binding(*port, binding.clone());
            }
            for s in subscriptions {
                scope.graph.subscribe(s.emitter, s.event_idx, s.subscriber);
            }
            scope.view.selected = to_selection.clone();
        }
        GraphStep::RemoveNode { node, .. } => {
            assert!(
                scope
                    .graph
                    .find_node(&node.id, NodeSearch::TopLevel)
                    .is_some(),
                "apply RemoveNode expects node to be present"
            );
            scope.remove_node(&node.id);
        }
        GraphStep::MoveSelection {
            node_moves,
            pin_moves,
            ..
        } => {
            for (id, _, to) in node_moves {
                if let Some(vn) = scope.view.view_nodes.by_key_mut(id) {
                    vn.pos = *to;
                }
            }
            for (port, _, to) in pin_moves {
                scope.view.pin_positions.insert(*port, *to);
            }
        }
        GraphStep::RenameNode { node_id, to, .. } => {
            scope
                .graph
                .find_node_mut(node_id, NodeSearch::TopLevel)
                .unwrap()
                .name = to.clone();
        }
        GraphStep::SetInput { input, to, .. } => {
            scope.graph.set_input_binding(*input, to.clone());
        }
        GraphStep::SetSelection { to, .. } => {
            scope.view.selected = to.clone();
        }
        GraphStep::RaiseNode {
            node_id, to_index, ..
        } => {
            scope.view.view_nodes.move_to_index(node_id, *to_index);
        }
        GraphStep::SetNodeProperty { node_id, to, .. } => {
            set_node_property(scope, node_id, *to);
        }
        GraphStep::DetachSubgraph { node_id, def, .. } => {
            scope.graph.subgraphs.add((**def).clone());
            scope
                .graph
                .find_node_mut(node_id, NodeSearch::TopLevel)
                .unwrap()
                .kind = NodeKind::Subgraph(SubgraphRef::Local(def.id));
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
        GraphStep::SetOutputPinned { output, to, .. } => set_output_pinned(scope, *output, *to),
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
        .find_node_mut(node_id, NodeSearch::TopLevel)
        .unwrap();
    match prop {
        NodeProperty::Disabled(v) => node.disabled = v,
        NodeProperty::RuntimeCache(v) => node.cache = v,
    }
}

/// Mark or clear whether one output port is pinned. Shared by `apply_graph`
/// (writes `to`) and `revert_graph` (writes `from`). Unpinning drops the
/// port's preview widget, so its selection membership goes with it — a
/// selected pin left in the set once the widget is gone would be dead state.
///
/// Pinning guarantees the port has a `pin_positions` entry: the GUI paths
/// (`PinUi::apply`'s Cmd+drag, `port_row`'s pin/unpin menu toggle) always
/// follow this with a `MoveSelection` that seeds the port's real on-screen
/// position, but a script driving `Intent::SetOutputPinned` directly has no
/// canvas geometry to compute one — so this seeds a zero fallback whenever
/// pinning would otherwise leave the port without one. Every pinned port is
/// thus guaranteed an explicit position no matter the caller, which is what
/// lets `build_step` treat a missing entry as a genuine bug rather than a
/// case to handle gracefully.
fn set_output_pinned(scope: &mut EditScope<'_>, output: OutputPort, pinned: bool) {
    scope.graph.set_output_pinned(output, pinned);
    if pinned {
        scope.view.pin_positions.entry(output).or_default();
    } else {
        scope.view.selected.remove(&SelectionKey::Pin(output));
    }
}

/// Backward apply: write the step's "from" half to `doc`. Pairs
/// with [`apply_step`]; calling one after the other restores the
/// graph to its pre-commit state.
pub(crate) fn revert_step(step: &UndoStep, doc: &mut Document, target: GraphRef) {
    match step {
        UndoStep::Doc(step) => revert_doc(step, doc),
        UndoStep::Graph(step) => with_scope(doc, target, |scope| revert_graph(step, scope)),
    }
}

/// Backward-apply a document-global step.
fn revert_doc(step: &DocStep, doc: &mut Document) {
    match step {
        DocStep::Dock { from, .. } => doc.layout = from.clone(),
        DocStep::RenameBoundaryPort {
            sub_id,
            side,
            idx,
            from,
            to,
        } => doc.rename_boundary_port(*sub_id, *side, *idx, to, from),
        DocStep::RenameSubgraph { id, from, .. } => {
            if let Some(def) = doc.graph.subgraphs.by_key_mut(id) {
                def.name = from.clone();
            }
        }
    }
}

/// Backward-apply a graph-scoped step against its resolved `EditScope`.
fn revert_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode { node, def, .. } => {
            scope.remove_node(&node.id);
            if let Some(def) = def {
                scope.graph.subgraphs.remove_by_key(&def.id);
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
            for (_, node) in nodes {
                scope.remove_node(&node.id);
            }
            scope.view.selected = from_selection.clone();
        }
        GraphStep::RemoveNode {
            view_node,
            node,
            bindings,
            subscriptions,
            pin_positions,
            was_selected,
        } => {
            let removed_node_id = node.id;
            assert!(
                scope
                    .graph
                    .find_node(&node.id, NodeSearch::TopLevel)
                    .is_none(),
                "revert RemoveNode expects removed node to be absent"
            );
            scope.graph.add(node.clone());
            scope.view.view_nodes.add(view_node.clone());
            scope.graph.restore_wiring(bindings, subscriptions);
            scope
                .view
                .pin_positions
                .extend(pin_positions.iter().copied());
            if *was_selected {
                scope
                    .view
                    .selected
                    .insert(SelectionKey::Node(removed_node_id));
            }
        }
        GraphStep::MoveSelection {
            node_moves,
            pin_moves,
            ..
        } => {
            for (id, from, _) in node_moves {
                if let Some(vn) = scope.view.view_nodes.by_key_mut(id) {
                    vn.pos = *from;
                }
            }
            for (port, from, _) in pin_moves {
                scope.view.pin_positions.insert(*port, *from);
            }
        }
        GraphStep::RenameNode { node_id, from, .. } => {
            scope
                .graph
                .find_node_mut(node_id, NodeSearch::TopLevel)
                .unwrap()
                .name = from.clone();
        }
        GraphStep::SetInput { input, from, .. } => {
            scope.graph.set_input_binding(*input, from.clone());
        }
        GraphStep::SetSelection { from, .. } => {
            scope.view.selected = from.clone();
        }
        GraphStep::RaiseNode {
            node_id,
            from_index,
            ..
        } => {
            scope.view.view_nodes.move_to_index(node_id, *from_index);
        }
        GraphStep::SetNodeProperty { node_id, from, .. } => {
            set_node_property(scope, node_id, *from);
        }
        GraphStep::DetachSubgraph {
            node_id,
            from_id,
            def,
        } => {
            scope
                .graph
                .find_node_mut(node_id, NodeSearch::TopLevel)
                .unwrap()
                .kind = NodeKind::Subgraph(SubgraphRef::Local(*from_id));
            scope.graph.subgraphs.remove_by_key(&def.id);
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
        GraphStep::SetOutputPinned {
            output,
            from,
            was_selected,
            ..
        } => {
            set_output_pinned(scope, *output, *from);
            // Re-pinning on undo doesn't itself restore selection (pinning
            // never auto-selects) — restore it explicitly when the pin was
            // selected before the edit that unpinned it.
            if *from && *was_selected {
                scope.view.selected.insert(SelectionKey::Pin(*output));
            }
        }
    }
}
