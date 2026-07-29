//! Read pre-mutation state from a [`Document`] and fold it with an
//! [`Intent`] (or a [`DocIntent`]) into a complete step — the
//! diff-capture half of the intent pipeline. Pure: never writes to the
//! graph.

use std::collections::HashSet;

use glam::Vec2;
use scenarium::{Binding, Graph, GraphId, GraphLink, Node, NodeId, NodeKind};

use crate::core::document::dock::DockOp;
use crate::core::document::{BoundarySide, Document, EditScopeRef, GraphRef};
use crate::core::edit::intent::types::{
    DetachedBoundaryPort, DocIntent, DocStep, GestureKey, GraphStep, Intent, NodeProperty, Refusal,
    UndoStep,
};
use crate::core::edit::intent::validate;

/// Read pre-mutation state from `doc` and fold it with `intent` into a
/// complete [`UndoStep`]. Pure — does not write to the graph.
///
/// This is the only gate between a caller and the document, so each arm
/// establishes the full precondition set its `apply` half assumes: an `Ok`
/// result is a proof that applying the step trips no assert on the way and
/// leaves the document passing [`Document::validate`]. Widgets only ever
/// violate the staleness half — they read the identities they emit out of
/// the live document — but a script's decoded payload reaches this same
/// entry with arbitrary contents.
///
/// [`Refusal::Quiet`] covers what a gesture spanning frames does normally:
/// the anchor node vanished, or the edit is refused by design.
/// [`Refusal::Invalid`] means the payload could never have applied.
/// (`MoveSelection` and `SetSelection` instead drop vanished members
/// individually rather than refusing the whole intent.)
pub(crate) fn build_step(
    intent: Intent,
    doc: &Document,
    target: GraphRef,
) -> Result<UndoStep, Refusal> {
    if let Intent::RenameBoundaryPort { side, idx, to } = intent {
        // Boundary ports only exist in a graph interior; the graph is
        // the active `Local` target's. Drop the rename otherwise.
        let GraphRef::Local(graph_id) = target else {
            return Err(Refusal::Quiet);
        };
        let from = doc
            .boundary_port_name(graph_id, side, idx)
            .ok_or(Refusal::Quiet)?
            .to_owned();
        return Ok(UndoStep::Doc(DocStep::RenameBoundaryPort {
            graph_id,
            side,
            idx,
            from,
            to,
        }));
    }
    if let Intent::AddBoundaryPort {
        side,
        name,
        data_type,
    } = intent
    {
        let GraphRef::Local(graph_id) = target else {
            return Err(Refusal::Quiet);
        };
        let interface = &doc.graph.find_graph(graph_id).ok_or(Refusal::Quiet)?;
        let idx = match side {
            BoundarySide::Input => interface.inputs.len(),
            BoundarySide::Output => interface.outputs.len(),
        };
        return Ok(UndoStep::Doc(DocStep::AddBoundaryPort {
            graph_id,
            side,
            idx,
            name,
            data_type,
        }));
    }
    if let Intent::RemoveBoundaryPort { side, idx } = intent {
        let GraphRef::Local(graph_id) = target else {
            return Err(Refusal::Quiet);
        };
        // Boundary snapshot/detach are *parent* methods (they sever the
        // owner's instance bindings too), so resolve the def's parent —
        // the root itself for a top-level def, an ancestor def otherwise.
        let parent = doc
            .graph
            .find_graph_parent(graph_id)
            .ok_or(Refusal::Quiet)?;
        let detached = match side {
            BoundarySide::Input => parent
                .snapshot_graph_input(graph_id, idx)
                .map(DetachedBoundaryPort::Input),
            BoundarySide::Output => parent
                .snapshot_graph_output(graph_id, idx)
                .map(DetachedBoundaryPort::Output),
        }
        .ok_or(Refusal::Quiet)?;
        return Ok(UndoStep::Doc(DocStep::RemoveBoundaryPort {
            graph_id,
            detached,
        }));
    }
    let EditScopeRef { graph, view } = doc.scope(target).ok_or(Refusal::Quiet)?;
    let step = match intent {
        Intent::RenameBoundaryPort { .. }
        | Intent::AddBoundaryPort { .. }
        | Intent::RemoveBoundaryPort { .. } => {
            unreachable!("interface edits are resolved against the interface above")
        }
        Intent::AddNode {
            pos,
            node_id,
            node,
            bindings,
        } => {
            let mut added = HashSet::new();
            validate::fresh_node_id(doc, node_id, &mut added)?;
            validate::finite_position(pos, "AddNode")?;
            validate::insertable_kind(graph, target, &node)?;
            validate::insertable_bindings(graph, &added, &bindings)?;
            GraphStep::AddNode {
                pos,
                node_id,
                node,
                graph: None,
                bindings,
            }
        }
        Intent::AddLocalGraph {
            pos,
            node_id,
            graph_id,
            def,
        } => {
            let mut added = HashSet::new();
            validate::fresh_node_id(doc, node_id, &mut added)?;
            validate::finite_position(pos, "AddLocalGraph")?;
            validate::fresh_local_graph(doc, graph_id, &def)?;
            // A second instance of one library entry shares the copy this
            // graph already holds rather than forking another — at which
            // point there is no definition to add and this *is* an instance.
            match def
                .origin
                .and_then(|origin| local_graph_from(graph, origin))
            {
                Some(existing) => instance_local_graph(graph, pos, node_id, existing)?,
                None => GraphStep::AddNode {
                    pos,
                    node_id,
                    node: Node::graph_instance(&def, GraphLink::Local(graph_id)),
                    bindings: def.ports().default_bindings(node_id).collect(),
                    graph: Some((graph_id, def)),
                },
            }
        }
        Intent::AddLocalGraphInstance {
            pos,
            node_id,
            graph_id,
        } => {
            let mut added = HashSet::new();
            validate::fresh_node_id(doc, node_id, &mut added)?;
            validate::finite_position(pos, "AddLocalGraphInstance")?;
            instance_local_graph(graph, pos, node_id, graph_id)?
        }
        Intent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        } => {
            let mut added = HashSet::with_capacity(nodes.len());
            for (pos, node_id, node) in &nodes {
                validate::fresh_node_id(doc, *node_id, &mut added)?;
                validate::finite_position(*pos, "DuplicateNodes")?;
                // A clone shares its original's local definition rather
                // than bringing one, so the link always resolves already.
                validate::insertable_kind(graph, target, node)?;
            }
            validate::insertable_bindings(graph, &added, &bindings)?;
            for subscription in &subscriptions {
                validate::present_node(
                    graph,
                    &added,
                    subscription.emitter,
                    "subscription emitter",
                )?;
                validate::present_node(
                    graph,
                    &added,
                    subscription.subscriber,
                    "subscription subscriber",
                )?;
            }
            let to_selection = nodes.iter().map(|(_, node_id, _)| *node_id).collect();
            GraphStep::DuplicateNodes {
                nodes,
                bindings,
                subscriptions,
                from_selection: view.selected.clone(),
                to_selection,
            }
        }
        Intent::RemoveNode { node_id } => {
            validate::live_node(graph, node_id, "RemoveNode")?;
            let detached = graph.snapshot_node(node_id).ok_or(Refusal::Quiet)?;
            // The node's own item with its paint-stack slot — ascending by
            // construction (enumerate).
            let item_placements = view
                .item_placements
                .iter()
                .enumerate()
                .filter(|(_, (key, _))| **key == node_id)
                .map(|(slot, (&key, &position))| (slot, key, position))
                .collect();
            let selected = view
                .selected
                .iter()
                .filter(|key| **key == node_id)
                .copied()
                .collect();
            GraphStep::RemoveNode {
                detached,
                item_placements,
                selected,
            }
        }
        Intent::MoveSelection { grabbed, moves } => {
            let mut placed = Vec::with_capacity(moves.len());
            for (key, to) in moves {
                validate::finite_position(to, "MoveSelection")?;
                // Drag-sourced (spans frames): a member whose item vanished
                // mid-gesture (node removed) drops quietly.
                let Some(&from) = view.item_placements.get(&key) else {
                    continue;
                };
                placed.push((key, from, to));
            }
            GraphStep::MoveSelection {
                grabbed,
                moves: placed,
            }
        }
        Intent::RenameNode { node_id, to } => GraphStep::RenameNode {
            from: validate::live_node(graph, node_id, "RenameNode")?
                .name
                .clone(),
            node_id,
            to,
        },
        Intent::SetInput { input, to } => {
            validate::live_node(graph, input.node_id, "SetInput destination")?;
            if let Some(Binding::Bind(src)) = &to {
                // A wire held across frames can outlive its producer, and a
                // script can name one that was never there; either way the
                // bind would leave the graph with a dangling edge.
                validate::live_node(graph, src.node_id, "SetInput producer")?;
                // Reject a bind that would close a data cycle: the planner
                // rejects a cyclic graph outright (`Error::CycleDetected`), so
                // the edit must never land. The GUI snap filter normally stops
                // this earlier; this is the authoritative guard covering every
                // binding path, including any that bypass the canvas.
                if graph.produces_cycle(src.node_id, input.node_id) {
                    return Err(Refusal::Quiet);
                }
            }
            GraphStep::SetInput {
                from: graph.bindings.get(&input).cloned(),
                input,
                to,
            }
        }
        Intent::SetSelection { to } => GraphStep::SetSelection {
            from: view.selected.clone(),
            // The rubber band snapshots identities when the drag starts, so
            // an interleaved undo can remove one before release; a script can
            // name an item that never existed. Keep the members that still
            // have a widget rather than recording a selection the view
            // can't render.
            to: to
                .into_iter()
                .filter(|key| view.item_placements.contains_key(key))
                .collect(),
        },
        Intent::Raise { key } => {
            let from_index = view
                .item_placements
                .get_index_of(&key)
                .ok_or(Refusal::Quiet)?;
            // Top of the stack is the last slot — painted last, drawn in front.
            let to_index = view.item_placements.len() - 1;
            GraphStep::Raise {
                key,
                from_index,
                to_index,
            }
        }
        Intent::SetNodeProperty { node_id, to } => {
            let node = validate::live_node(graph, node_id, "SetNodeProperty")?;
            // Capture the *same* property's current value as `from` for revert.
            let from = match to {
                NodeProperty::Disabled(_) => NodeProperty::Disabled(node.disabled),
                NodeProperty::RuntimeCache(_) => NodeProperty::RuntimeCache(node.cache),
            };
            GraphStep::SetNodeProperty { node_id, from, to }
        }
        Intent::DetachGraph { node_id } => {
            let NodeKind::Graph(GraphLink::Local(from_id)) =
                validate::live_node(graph, node_id, "DetachGraph")?.kind
            else {
                return Err(Refusal::Quiet); // not a local graph instance — nothing to fork
            };
            let to_id = GraphId::unique();
            let mut copy = graph
                .graphs
                .get(&from_id)
                .ok_or(Refusal::Quiet)?
                .clone_mapped();
            copy.origin = None;
            GraphStep::DetachGraph {
                node_id,
                from_id,
                to_id,
                graph: Box::new(copy),
            }
        }
        Intent::SetViewport { to } => {
            if !to.is_valid() {
                return Err(Refusal::Invalid(
                    "viewport needs finite pan and positive finite zoom".to_owned(),
                ));
            }
            GraphStep::SetViewport {
                from: view.viewport,
                to,
            }
        }
        Intent::SetSubscription {
            emitter,
            event_idx,
            subscriber,
            subscribe,
        } => {
            if emitter.is_nil() || subscriber.is_nil() {
                return Err(Refusal::Invalid(
                    "SetSubscription carries a nil node id".to_owned(),
                ));
            }
            // A subscribe needs both endpoints present; a stale drag onto a
            // vanished node drops rather than recording a dangling subscription.
            // An unsubscribe of a vanished node no-ops naturally (nothing is
            // subscribed → from == to == false), so it needs no existence check.
            if subscribe {
                validate::live_node(graph, emitter, "SetSubscription emitter")?;
                validate::live_node(graph, subscriber, "SetSubscription subscriber")?;
            }
            GraphStep::SetSubscription {
                from: graph.is_subscribed(emitter, event_idx, subscriber),
                to: subscribe,
                emitter,
                event_idx,
                subscriber,
            }
        }
    };
    Ok(UndoStep::Graph(step))
}

/// The [`build_step`] counterpart for the mutations no graph owns: fold
/// `intent` with the document-wide state it overwrites into a complete
/// [`DocStep`]. Takes no target, because there is none to take.
///
/// Same gate contract as `build_step` — an `Ok` is a proof that applying
/// the step is sound — but the preconditions are thinner: a dock op is
/// applied to a *copy* of the layout here and recorded as the
/// before/after pair, so an op the layout refuses simply lands as
/// `from == to` and drops out through the no-op filter.
pub(crate) fn build_doc_step(intent: DocIntent, doc: &Document) -> Result<DocStep, Refusal> {
    match intent {
        DocIntent::Dock(op) => {
            let key = match op {
                DockOp::ActivateTab { .. } => Some(GestureKey::TabSwitch),
                DockOp::SetRatio { split, .. } => Some(GestureKey::DockResize(split)),
                DockOp::CloseTab { .. } | DockOp::MoveTab { .. } => None,
            };
            let structural = matches!(op, DockOp::MoveTab { .. });
            let from = doc.layout.clone();
            let mut to = from.clone();
            to.apply(op);
            Ok(DocStep::Dock {
                from,
                to,
                key,
                structural,
            })
        }
        DocIntent::RenameGraph { id, to } => {
            let from = doc.graph.find_graph(id).ok_or(Refusal::Quiet)?.name.clone();
            Ok(DocStep::RenameGraph { id, from, to })
        }
    }
}

/// Instance the definition `graph_id` names, seeding the const defaults its
/// interface declares. The one place a bare instance is built, so reusing an
/// existing copy and picking one out of the palette cannot drift apart.
///
/// Only the target graph's *own* definitions resolve through a
/// `GraphLink::Local` raised over it — the same rule
/// [`validate::insertable_kind`] enforces for a caller-built node.
fn instance_local_graph(
    graph: &Graph,
    pos: Vec2,
    node_id: NodeId,
    graph_id: GraphId,
) -> Result<GraphStep, Refusal> {
    let Some(definition) = graph.graphs.get(&graph_id) else {
        return Err(Refusal::Invalid(format!(
            "cannot instance local graph {graph_id:?}, which the target graph doesn't hold"
        )));
    };
    Ok(GraphStep::AddNode {
        pos,
        node_id,
        node: Node::graph_instance(definition, GraphLink::Local(graph_id)),
        graph: None,
        bindings: definition.ports().default_bindings(node_id).collect(),
    })
}

/// This graph's own copy of the library entry `origin`, if it holds one.
fn local_graph_from(graph: &Graph, origin: GraphId) -> Option<GraphId> {
    graph
        .graphs
        .iter()
        .find(|(_, def)| def.origin == Some(origin))
        .map(|(id, _)| *id)
}
