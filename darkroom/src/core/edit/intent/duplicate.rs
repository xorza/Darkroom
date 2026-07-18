//! Editor-side [`Intent::DuplicateNodes`] construction from a selection, and
//! the [`Intent`]s that remove one. Kept here rather than on `Document` —
//! that's the persisted model; intent construction is editing machinery.

use std::collections::{BTreeSet, HashMap};

use glam::Vec2;
use scenarium::{Binding, InputPort, NodeId, NodeSearch, Subscription};

use crate::core::document::{Document, EditScopeRef, GraphRef, GraphView, ItemRef};
use crate::core::edit::intent::types::Intent;

/// World-space offset applied to duplicated nodes so the copies don't
/// land exactly on top of their originals.
pub(crate) const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// The `NodeId`s among `view`'s selection, dropping pin-preview keys (which
/// carry no node identity). Shared by the Ctrl+D duplicate path
/// ([`build_duplicate_intent`]) and the node context menu's duplicate action
/// (`Editor::apply_node_menu_action`), which both need "just the selected
/// nodes" from a selection that can also hold pinned-output keys.
pub(crate) fn selected_node_ids(view: &GraphView) -> BTreeSet<NodeId> {
    view.selected
        .iter()
        .filter_map(|k| match k {
            ItemRef::Node(id) => Some(*id),
            ItemRef::Pin(_) => None,
        })
        .collect()
}

/// Build an [`Intent::DuplicateNodes`] for `target`'s current selection.
/// Thin wrapper over [`build_duplicate_intent_for`] with the selected node
/// bodies (pinned-output previews carry no node identity to clone, so
/// they're filtered out) and incoming (external) wires dropped — the Ctrl+D
/// path.
pub(crate) fn build_duplicate_intent(doc: &Document, target: GraphRef) -> Option<Intent> {
    let EditScopeRef { view, .. } = doc.scope(target)?;
    let node_ids = selected_node_ids(view);
    if node_ids.is_empty() {
        return None;
    }
    build_duplicate_intent_for(doc, target, &node_ids, false)
}

/// Build an [`Intent::DuplicateNodes`] cloning `node_ids` in `target`: each
/// node gets a fresh id and an offset position, const-value bindings copy
/// verbatim, and the data + event connections *among* `node_ids` are
/// recreated against the clones. A `Bind` whose source is *outside* the set
/// is dropped unless `include_incoming` is set, in which case the clone
/// keeps the wire pointing at the original external producer. `None` when
/// `node_ids` is empty or the target doesn't resolve. Reads the document to
/// assemble the intent — editor-operation construction, kept with the rest
/// of the intent machinery rather than on the `Document` model.
pub(crate) fn build_duplicate_intent_for(
    doc: &Document,
    target: GraphRef,
    node_ids: &BTreeSet<NodeId>,
    include_incoming: bool,
) -> Option<Intent> {
    let EditScopeRef { graph, view } = doc.scope(target)?;
    if node_ids.is_empty() {
        return None;
    }

    let mut id_map: HashMap<NodeId, NodeId> = HashMap::new();
    let mut nodes = Vec::new();
    for old_id in node_ids {
        let Some(node) = graph.find_node(old_id, NodeSearch::TopLevel) else {
            continue;
        };
        let new_id = NodeId::unique();
        id_map.insert(*old_id, new_id);
        let clone = node.clone();
        let pos = view
            .view_items
            .by_key(&ItemRef::Node(*old_id))
            .expect("view holds a position for every graph node")
            .pos
            + DUPLICATE_OFFSET;
        nodes.push((pos, new_id, clone));
    }

    // Each cloned node's own input ports. Const/None copy verbatim; a `Bind`
    // to a source inside the set is remapped to that source's clone. A `Bind`
    // to an *external* source is dropped — unless `include_incoming`, where
    // the clone keeps the wire to the original producer.
    let mut bindings = Vec::new();
    for old_id in node_ids {
        for entry in graph.bindings_touching(*old_id) {
            let port = entry.port;
            let binding = entry.binding;
            if port.node_id != *old_id {
                continue;
            }
            let new_binding = match binding {
                Binding::Bind(src) => match id_map.get(&src.node_id) {
                    Some(&new_src) => Binding::bind(new_src, src.port_idx),
                    None if include_incoming => Binding::Bind(src),
                    None => continue,
                },
                other => other,
            };
            bindings.push((InputPort::new(id_map[old_id], port.port_idx), new_binding));
        }
    }

    // Event subscriptions internal to the set.
    let mut subscriptions = Vec::new();
    for s in graph.subscriptions() {
        if let (Some(&emitter), Some(&subscriber)) =
            (id_map.get(&s.emitter), id_map.get(&s.subscriber))
        {
            subscriptions.push(Subscription {
                emitter,
                event_idx: s.event_idx,
                subscriber,
            });
        }
    }

    Some(Intent::DuplicateNodes {
        nodes,
        bindings,
        subscriptions,
    })
}

/// The intents that remove every member of `selected`: a node key becomes
/// `RemoveNode`, a pin key becomes an unpin (`SetOutputPinned { pinned:
/// false }`) — deleting a preview widget just unpins its port rather than
/// touching the node it lives on. Shared by the Delete/Backspace shortcut
/// and the node context menu's "Remove".
pub(crate) fn remove_selection_intents(selected: &BTreeSet<ItemRef>) -> Vec<Intent> {
    selected
        .iter()
        .map(|key| match *key {
            ItemRef::Node(node_id) => Intent::RemoveNode { node_id },
            ItemRef::Pin(port) => Intent::SetOutputPinned {
                output: port,
                pinned: false,
            },
        })
        .collect()
}
