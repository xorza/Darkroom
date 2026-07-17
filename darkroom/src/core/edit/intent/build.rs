//! Read pre-mutation state from a [`Document`] and fold it with an
//! [`Intent`] into a complete [`UndoStep`] — the diff-capture half of the
//! intent pipeline. Pure: never writes to the graph.

use scenarium::{Graph, Node, NodeKind, NodeSearch};
use scenarium::{SubgraphDef, SubgraphRef};

use crate::core::document::dock::DockOp;
use crate::core::document::{Document, EditScopeRef, GraphRef, ItemRef};
use crate::core::edit::intent::types::{
    DocStep, GestureKey, GraphStep, Intent, NodeProperty, UndoStep,
};

/// Read pre-mutation state from `doc` and fold it with `intent`
/// into a complete [`UndoStep`]. Pure — does not write to the graph.
/// Returns `None` when the intent targets a node that no longer exists
/// (e.g. a `RemoveNode`/`SetInput` whose anchor lingered one frame past a
/// `RemoveNode` applied earlier in the same frame) or carries an invalid
/// viewport. Callers should treat a `None` result as "invalid or stale intent,
/// drop it". (`MoveSelection` instead skips vanished nodes/pins individually
/// rather than dropping the whole batch.)
pub(crate) fn build_step(intent: Intent, doc: &Document, target: GraphRef) -> Option<UndoStep> {
    // Document-global intents don't resolve a graph scope.
    if let Intent::Dock(op) = intent {
        let key = match op {
            DockOp::ActivateTab { .. } => Some(GestureKey::TabSwitch),
            DockOp::SetRatio { split, .. } => Some(GestureKey::DockResize(split)),
            DockOp::CloseTab { .. } | DockOp::MoveTab { .. } => None,
        };
        let structural = matches!(op, DockOp::MoveTab { .. });
        let from = doc.layout.clone();
        let mut to = from.clone();
        to.apply(op);
        // Refused/degenerate ops leave `to == from`; the is_noop filter
        // drops the step.
        return Some(UndoStep::Doc(DocStep::Dock {
            from,
            to,
            key,
            structural,
        }));
    }
    if let Intent::RenameSubgraph { id, to } = intent {
        let from = doc.graph.subgraphs.by_key(&id)?.name.clone();
        return Some(UndoStep::Doc(DocStep::RenameSubgraph { id, from, to }));
    }
    if let Intent::RenameBoundaryPort { side, idx, to } = intent {
        // Boundary ports only exist in a subgraph interior; the def is
        // the active `Local` target's. Drop the rename otherwise.
        let GraphRef::Local(sub_id) = target else {
            return None;
        };
        let from = doc.boundary_port_name(sub_id, side, idx)?.to_owned();
        return Some(UndoStep::Doc(DocStep::RenameBoundaryPort {
            sub_id,
            side,
            idx,
            from,
            to,
        }));
    }
    let EditScopeRef { graph, view } = doc.scope(target)?;
    let step = match intent {
        Intent::Dock(_) | Intent::RenameBoundaryPort { .. } | Intent::RenameSubgraph { .. } => {
            unreachable!("document-global intents handled above")
        }
        Intent::AddNode {
            pos,
            mut node,
            def,
            bindings,
        } => {
            let def = reuse_local_subgraph(graph, &mut node, def);
            GraphStep::AddNode {
                pos,
                node,
                def,
                bindings,
            }
        }
        Intent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        } => {
            let to_selection = nodes
                .iter()
                .map(|(_, node)| ItemRef::Node(node.id))
                .collect();
            GraphStep::DuplicateNodes {
                nodes,
                bindings,
                subscriptions,
                from_selection: view.selected.clone(),
                to_selection,
            }
        }
        Intent::RemoveNode { node_id } => {
            let detached = graph.snapshot_node(node_id)?;
            // The node's own item plus its pinned outputs', each with its
            // paint-stack slot — ascending by construction (enumerate).
            let view_items = view
                .view_items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.key.belongs_to(node_id))
                .map(|(slot, item)| (slot, item.clone()))
                .collect();
            let selected = view
                .selected
                .iter()
                .filter(|key| key.belongs_to(node_id))
                .copied()
                .collect();
            GraphStep::RemoveNode {
                detached,
                view_items,
                selected,
            }
        }
        Intent::MoveSelection { grabbed, moves } => {
            // Drag-sourced (spans frames): a member whose item vanished
            // mid-gesture (node removed, port unpinned) drops quietly.
            let moves = moves
                .into_iter()
                .filter_map(|(key, to)| {
                    let item = view.view_items.by_key(&key)?;
                    Some((key, item.pos, to))
                })
                .collect();
            GraphStep::MoveSelection { grabbed, moves }
        }
        Intent::RenameNode { node_id, to } => GraphStep::RenameNode {
            from: graph
                .find_node(&node_id, NodeSearch::TopLevel)?
                .name
                .clone(),
            node_id,
            to,
        },
        Intent::SetInput { input, to } => {
            graph.find_node(&input.node_id, NodeSearch::TopLevel)?;
            GraphStep::SetInput {
                from: graph.input_binding(input),
                input,
                to,
            }
        }
        Intent::SetSelection { to } => GraphStep::SetSelection {
            from: view.selected.clone(),
            to,
        },
        Intent::Raise { key } => {
            let from_index = view.view_items.index_of_key(&key)?;
            // Top of the stack is the last slot — painted last, drawn in front.
            let to_index = view.view_items.len() - 1;
            GraphStep::Raise {
                key,
                from_index,
                to_index,
            }
        }
        Intent::SetNodeProperty { node_id, to } => {
            let node = graph.find_node(&node_id, NodeSearch::TopLevel)?;
            // Capture the *same* property's current value as `from` for revert.
            let from = match to {
                NodeProperty::Disabled(_) => NodeProperty::Disabled(node.disabled),
                NodeProperty::RuntimeCache(_) => NodeProperty::RuntimeCache(node.cache),
            };
            GraphStep::SetNodeProperty { node_id, from, to }
        }
        Intent::DetachSubgraph { node_id } => {
            let NodeKind::Subgraph(SubgraphRef::Local(from_id)) =
                graph.find_node(&node_id, NodeSearch::TopLevel)?.kind
            else {
                return None; // not a local subgraph instance — nothing to fork
            };
            let mut copy = graph.subgraphs.by_key(&from_id)?.fresh_copy();
            copy.origin = None; // detach severs the library lineage
            GraphStep::DetachSubgraph {
                node_id,
                from_id,
                def: Box::new(copy),
            }
        }
        Intent::SetViewport { to } => {
            if !to.is_valid() {
                return None;
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
            // A subscribe needs both endpoints present; a stale drag onto a
            // vanished node drops rather than recording a dangling subscription.
            // An unsubscribe of a vanished node no-ops naturally (nothing is
            // subscribed → from == to == false), so it needs no existence check.
            if subscribe {
                graph.find_node(&emitter, NodeSearch::TopLevel)?;
                graph.find_node(&subscriber, NodeSearch::TopLevel)?;
            }
            GraphStep::SetSubscription {
                from: graph.is_subscribed(emitter, event_idx, subscriber),
                to: subscribe,
                emitter,
                event_idx,
                subscriber,
            }
        }
        Intent::SetOutputPinned { output, pinned } => {
            // From the GUI this only ever targets a port rendered in the
            // current frame's Scene — but `core::script::register_mutations`
            // also reaches this variant directly, unchecked, from a script's
            // generic `apply()`/`apply_all()`, so a stale or bogus `node_id`
            // must drop like every other intent here, not assert.
            graph.find_node(&output.node_id, NodeSearch::TopLevel)?;
            let key = ItemRef::Pin(output);
            // Present iff currently pinned; captured so reverting an unpin
            // puts the widget back in its exact paint-stack slot.
            let prior_slot = view
                .view_items
                .index_of_key(&key)
                .map(|slot| (slot, view.view_items[slot].pos));
            GraphStep::SetOutputPinned {
                output,
                from: graph.is_output_pinned(output),
                to: pinned,
                was_selected: view.selected.contains(&key),
                prior_slot,
            }
        }
    };
    Some(UndoStep::Graph(step))
}

/// Library subgraphs are localized on instance: the new-node menu drops
/// a fresh `Local` copy tagged with the library def it came from
/// (`origin`). If `graph` already holds a local def from the same
/// library source, re-point `node` at that existing def and drop the
/// duplicate copy — one editable local def, many instances. Otherwise
/// return `def` untouched (the first instance materializes the copy).
fn reuse_local_subgraph(
    graph: &Graph,
    node: &mut Node,
    def: Option<Box<SubgraphDef>>,
) -> Option<Box<SubgraphDef>> {
    let def = def?;
    // No library lineage (hand-authored local def, e.g. collapse-to-
    // subgraph) → always its own copy; nothing to dedup against.
    let Some(origin) = def.origin else {
        return Some(def);
    };
    match graph.subgraphs.iter().find(|d| d.origin == Some(origin)) {
        Some(existing) => {
            node.kind = NodeKind::Subgraph(SubgraphRef::Local(existing.id));
            None
        }
        None => Some(def),
    }
}
