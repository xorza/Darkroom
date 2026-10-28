//! The widget-id vocabulary of the canvas — a node's subtree and a tie's band — plus the
//! queries keyed by it.
//!
//! Every id under a node is reconstructible from its domain coordinates — a
//! [`NodeId`], and for the per-port widgets a [`PortRef`] — so any pass can
//! `response_for` last frame's state without threading a cache. That is what
//! lets the prepass read clicks before the record, and the breaker and the
//! geometry rebuild probe arranged rects before the widgets round-trip their
//! own responses.
//!
//! A leaf, apart from the modules that record the widgets: the geometry rebuild, the gestures,
//! the test harness, and the inspector all derive ids here for widgets they never record.

use palantir::{Ui, WidgetId};
use scenarium::NodeId;

use scenarium::{EventPort, InputPort};

use crate::core::document::graph_view::GraphView;
use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::gui::port_ref::PortRef;

use crate::gui::widgets::hover::Hover;

/// A node-keyed widget id under `tag`.
pub(super) fn of_node(tag: &'static str, node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.node", tag, node_id))
}

/// A port-keyed widget id — [`of_node`] for the per-port widgets, keyed by side
/// and index as well.
pub(super) fn of_port(tag: &'static str, port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "darkroom.graph.node",
        tag,
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// An event-keyed widget id — [`of_node`] for the per-event widgets, keyed by
/// the event's index as well.
pub(super) fn of_event(tag: &'static str, event: EventPort) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.node", tag, event.node_id, event.event_idx))
}

/// The node's outer body panel — probed by the wire breaker and the
/// geometry rebuild for last frame's arranged rect, before the panel's own
/// response round-trips.
pub(crate) fn body(node_id: NodeId) -> WidgetId {
    of_node("body", node_id)
}

/// A node's footer, under its ports. No widget takes a press in the middle of it, past the run
/// status at its left, so a press there grabs the body.
pub(crate) fn footer(node_id: NodeId) -> WidgetId {
    of_node("footer", node_id)
}

/// A node's inline title-rename editor *and* its idle label, so the same id is
/// recorded across the label⇄editor swap. Polled by [`drag_handles`] to drag
/// the node by its title (the idle label senses `DRAG`).
pub(crate) fn rename(node_id: NodeId) -> WidgetId {
    of_node("title_rename", node_id)
}

pub(crate) fn port_circle(port: PortRef) -> WidgetId {
    of_port("port_circle", port)
}

/// An input port's inline const editor (text field, checkbox, or file-pick
/// button).
pub(super) fn const_editor(input: InputPort) -> WidgetId {
    of_port("const_editor", input.into())
}

/// An input port's cell (circle + label). Answers a double-click on the
/// *label* area — the circle has its own [`port_circle`] and consumes
/// hits over its own rect.
pub(super) fn input_cell(port: PortRef) -> WidgetId {
    of_port("input_cell", port)
}

/// An event port glyph. A separate id space from data ports
/// ([`port_circle`]) because events are indexed independently of outputs.
pub(super) fn event_glyph(event: EventPort) -> WidgetId {
    of_event("event_glyph", event)
}

/// Stable id for a node's event-subscription pin. Keyed on the node (a
/// subscription is whole-node, not per-port), so `CanvasGeometry` /
/// `SubscriptionUi` reconstruct it to poll the pin's geometry as a wire
/// drop target.
pub(super) fn subscription_glyph(node_id: NodeId) -> WidgetId {
    of_node("subscription_glyph", node_id)
}

/// Stable id for a preview's value area. Reconstructible from the node, so the
/// canvas-level scan can read last frame's click without a cache.
pub(crate) fn preview_image(node_id: NodeId) -> WidgetId {
    of_node("preview_image", node_id)
}

/// Stable id for a node's inspect button in the header.
pub(super) fn inspect_button(node_id: NodeId) -> WidgetId {
    of_node("inspect_button", node_id)
}

/// The hover and press target of `tie`'s band.
pub(super) fn tie_body(tie: TieId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.tie", "body", tie))
}

/// The knob of a selected `tie`.
pub(super) fn tie_knob(tie: TieId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.tie", "knob", tie))
}

/// Every widget whose drag moves `node_id`'s body, in the order `NodeUi::draw_node` tries them.
///
/// Not just the body panel: the header title doubles as a drag handle — its
/// idle label senses `DRAG` and **swallows the press**, so the body never sees
/// one latched there. (While renaming, the title is a `TextEdit` with no
/// `DRAG`, so it can't fire mid-edit.)
///
/// Deliberately *curated*, not "everything under the node". Port circles,
/// header chips, and the inline const editors are all inside the body panel and
/// all latch a drag of their own once the pointer travels — palantir's drag
/// latch ignores `Sense`, so even a `Sense::CLICK` port circle reports one.
/// Each of those owns its own gesture (a wire, a chip click, a text drag), so a
/// subtree-wide "did anything in here start a drag" would wrongly move the node
/// along with them. Palantir's tab chips carry the same shape for the same
/// reason.
pub(super) fn drag_handles(node_id: NodeId) -> impl Iterator<Item = WidgetId> {
    [body(node_id), rename(node_id)].into_iter()
}

/// Pointer-over-node for hover-reveal affordances (the const-editor chips).
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
pub(crate) fn hover(ui: &Ui, node_id: NodeId) -> Hover {
    Hover::from(ui.is_hover_within(body(node_id)))
}

/// Whether a hovered tie lights `wire`: the pointer is over a tie on its route, as a hovered
/// endpoint lights a wire.
pub(super) fn lit_by_tie(ui: &Ui, view: &GraphView, wire: WireKey) -> bool {
    view.routes.get(&wire).is_some_and(|route| {
        route
            .iter()
            .any(|&tie| ui.response_for(tie_body(tie)).hovered())
    })
}
