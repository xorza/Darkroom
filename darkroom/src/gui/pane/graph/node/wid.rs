//! The widget-id vocabulary of a node's subtree, plus the queries keyed by it.
//!
//! Every id under a node is reconstructible from its domain coordinates — a
//! [`NodeId`], and for the per-port widgets a [`PortRef`] — so any pass can
//! `response_for` last frame's state without threading a cache. That is what
//! lets the prepass read clicks before the record, and the breaker and the
//! geometry rebuild probe arranged rects before the widgets round-trip their
//! own responses.
//!
//! Kept out of the node module proper because these are shared vocabulary
//! rather than [`NodeUI`](crate::gui::pane::graph::node::NodeUI) state: the
//! geometry rebuild, the test harness, and the inspector all derive ids here
//! for widgets they never record.

use palantir::{Ui, WidgetId};
use scenarium::NodeId;

use crate::core::document::PortRef;

/// A node-keyed widget id under `tag`.
pub(super) fn node(tag: &'static str, node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node", tag, node_id))
}

/// A port-keyed widget id — [`node`] for the per-port widgets, keyed by side
/// and index as well.
pub(super) fn port(tag: &'static str, port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node",
        tag,
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// The node's outer body panel — probed by the connection breaker and the
/// geometry rebuild for last frame's arranged rect, before the panel's own
/// response round-trips.
pub(crate) fn body(node_id: NodeId) -> WidgetId {
    node("body", node_id)
}

/// A node's inline title-rename editor *and* its idle label, so the same id is
/// recorded across the label⇄editor swap. Polled by [`drag_handles`] to drag
/// the node by its title (the idle label senses `DRAG`).
pub(super) fn rename(node_id: NodeId) -> WidgetId {
    node("title_rename", node_id)
}

/// Every widget whose drag moves `node_id`'s body, in the order
/// [`NodeWidget::show`](crate::gui::pane::graph::node::widget::NodeWidget::show)
/// tries them.
///
/// Not just the body panel: the header title doubles as a drag handle — its
/// idle label senses `DRAG` and **swallows the press**, so the body never sees
/// one latched there. (While renaming, the title is a `TextEdit` with no
/// `DRAG`, so it can't fire mid-edit.)
///
/// Deliberately *curated*, not "everything under the node". Port circles,
/// header chips, and the inline value editors are all inside the body panel and
/// all latch a drag of their own once the pointer travels — palantir's drag
/// latch ignores `Sense`, so even a `Sense::CLICK` port circle reports one.
/// Each of those owns its own gesture (a wire, a chip click, a text drag), so a
/// subtree-wide "did anything in here start a drag" would wrongly move the node
/// along with them. Palantir's tab chips carry the same shape for the same
/// reason.
pub(super) fn drag_handles(node_id: NodeId) -> impl Iterator<Item = WidgetId> {
    [body(node_id), rename(node_id)].into_iter()
}

/// Pointer-over-node for hover-reveal affordances (the value-editor chips).
/// The body response's own `hovered` flag misses most of the node's area —
/// ports, chips, and editors capture the hit — so this asks whether the hover
/// *target* sits anywhere in the node's subtree. Target-derived (not a raw
/// `pointer_pos` rect test) on purpose: it can only change when the hover
/// target changes, which is exactly when a repaint is already scheduled — no
/// `MOVE` subscription needed — and it's occlusion-aware (a panel stacked over
/// the node wins the pointer).
///
/// Resolved against *last* frame's hover target and cascade, so the answer
/// doesn't depend on where in this frame's record it is asked — which is what
/// lets the record pass settle it at the node body — one
/// [`NodeCtx::with_hover`](crate::gui::graph_ctx::node_ctx::NodeCtx::with_hover)
/// per node — before the subtree that reads it has recorded.
pub(crate) fn hovered(ui: &Ui, node_id: NodeId) -> bool {
    ui.hover_within(body(node_id))
}
