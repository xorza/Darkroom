//! Read pre-mutation state from a [`Document`] and fold it with an
//! [`GraphIntent`] into a complete step — the
//! diff-capture half of the intent pipeline. Pure: never writes to the
//! graph.

use std::collections::HashSet;

use scenarium::Binding;

use crate::core::document::Document;
use crate::core::edit::intent::types::{GraphIntent, NodeProperty, Refusal, UndoStep};
use crate::core::edit::intent::validate;

/// Read pre-mutation state from `doc` and fold it with `intent` into a
/// complete [`UndoStep`]. Pure — does not write to the graph.
///
/// This is the only gate between a caller and the document, so each arm
/// establishes the full precondition set its `apply` half assumes: an `Ok`
/// result is a proof that applying the step trips no assert on the way and
/// leaves the document passing [`Document::validate`]. Widgets only ever
/// violate the staleness half, since they read the identities they emit out
/// of the live document; anything else reaching here is a bug.
///
/// [`Refusal::Quiet`] covers what a gesture spanning frames does normally:
/// the anchor node vanished, or the edit is refused by design.
/// [`Refusal::Invalid`] means the payload could never have applied.
/// (`MoveSelection` and `SetSelection` instead drop vanished members
/// individually rather than refusing the whole intent.)
pub(crate) fn build_step(intent: GraphIntent, doc: &Document) -> Result<UndoStep, Refusal> {
    let (graph, view) = (&doc.graph, &doc.main_view);
    let step = match intent {
        GraphIntent::AddNode {
            pos,
            node_id,
            node,
            bindings,
        } => {
            let mut added = HashSet::new();
            validate::fresh_node_id(doc, node_id, &mut added)?;
            validate::finite_position(pos, "AddNode")?;
            validate::insertable_kind(graph, &node)?;
            validate::insertable_bindings(graph, &added, &bindings)?;
            UndoStep::AddNode {
                pos,
                node_id,
                node,
                bindings,
            }
        }
        GraphIntent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        } => {
            let mut added = HashSet::with_capacity(nodes.len());
            for (pos, node_id, node) in &nodes {
                validate::fresh_node_id(doc, *node_id, &mut added)?;
                validate::finite_position(*pos, "DuplicateNodes")?;
                validate::insertable_kind(graph, node)?;
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
            UndoStep::DuplicateNodes {
                nodes,
                bindings,
                subscriptions,
                from_selection: view.selected.clone(),
                to_selection,
            }
        }
        GraphIntent::RemoveNode { node_id } => {
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
            UndoStep::RemoveNode {
                detached,
                item_placements,
                selected,
            }
        }
        GraphIntent::MoveSelection { grabbed, moves } => {
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
            UndoStep::MoveSelection {
                grabbed,
                moves: placed,
            }
        }
        GraphIntent::RenameNode { node_id, to } => UndoStep::RenameNode {
            from: validate::live_node(graph, node_id, "RenameNode")?
                .name
                .clone(),
            node_id,
            to,
        },
        GraphIntent::SetInput { input, to } => {
            validate::live_node(graph, input.node_id, "SetInput destination")?;
            if let Some(Binding::Bind(src)) = &to {
                // A wire held across frames can outlive its producer, and
                // the bind would leave the graph with a dangling edge.
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
            UndoStep::SetInput {
                from: graph.bindings.get(&input).cloned(),
                input,
                to,
            }
        }
        GraphIntent::SetSelection { to } => UndoStep::SetSelection {
            from: view.selected.clone(),
            // The rubber band snapshots identities when the drag starts, so
            // an interleaved undo can remove one before release. Keep the
            // members that still have a widget rather than recording a
            // selection the view can't render.
            to: to
                .into_iter()
                .filter(|key| view.item_placements.contains_key(key))
                .collect(),
        },
        GraphIntent::Raise { key } => {
            let from_index = view
                .item_placements
                .get_index_of(&key)
                .ok_or(Refusal::Quiet)?;
            // Top of the stack is the last slot — painted last, drawn in front.
            let to_index = view.item_placements.len() - 1;
            UndoStep::Raise {
                key,
                from_index,
                to_index,
            }
        }
        GraphIntent::SetNodeProperty { node_id, to } => {
            let node = validate::live_node(graph, node_id, "SetNodeProperty")?;
            // Capture the *same* property's current value as `from` for revert.
            let from = match to {
                NodeProperty::Disabled(_) => NodeProperty::Disabled(node.disabled),
                NodeProperty::RuntimeCache(_) => NodeProperty::RuntimeCache(node.cache),
            };
            UndoStep::SetNodeProperty { node_id, from, to }
        }
        GraphIntent::SetViewport { to } => {
            if !to.is_valid() {
                return Err(Refusal::Invalid(
                    "viewport needs finite pan and positive finite zoom".to_owned(),
                ));
            }
            UndoStep::SetViewport {
                from: view.viewport,
                to,
            }
        }
        GraphIntent::SetSubscription {
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
            UndoStep::SetSubscription {
                from: graph.is_subscribed(emitter, event_idx, subscriber),
                to: subscribe,
                emitter,
                event_idx,
                subscriber,
            }
        }
    };
    Ok(step)
}
