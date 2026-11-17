//! The widget-id vocabulary of the canvas — a node's subtree and a tie's band — plus the
//! queries keyed by it.
//!
//! Every id under a node is reconstructible from its domain coordinates (the pane's [`GraphId`]
//! and a [`NodeId`], and for the per-port widgets a [`PortRef`]), so any pass can `response_for` last frame's state
//! without threading a cache. That lets the input stage read clicks before the record, and
//! the breaker and the geometry rebuild probe arranged rects before the widgets round-trip
//! their own responses.
//!
//! A leaf, apart from the modules that record the widgets: the geometry rebuild, the gestures,
//! the test harness, and the inspector all derive ids here for widgets they never record.
//!
//! # Decisions
//!
//! - **An authored thing is keyed by its graph and its node id, a run-time thing by its program
//!   id**, as a copy of a definition keeps its node ids, so two open panes can hold the same one.

use palantir::{Ui, WidgetId};
use scenarium::{GraphId, NodeId, TriggerPort};

use scenarium::{EventPort, InputPort};

use crate::core::document::graph_view::GraphView;
use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::gui::port_ref::PortRef;

use crate::gui::theme::hover::Hover;

/// A node-keyed widget id under `tag`, in the pane of `graph`: a copy of a definition keeps its
/// node ids, so two panes can show a node under one id.
pub(super) fn of_node(tag: &'static str, graph: GraphId, node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.node", graph, tag, node_id))
}

/// A port-keyed widget id — [`of_node`] for the per-port widgets, keyed by side
/// and port id as well.
pub(super) fn of_port(tag: &'static str, graph: GraphId, port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "darkroom.graph.node",
        graph,
        tag,
        port.node_id,
        port.kind as u8,
        port.port,
    ))
}

/// An event-keyed widget id — [`of_node`] for the per-event widgets, keyed by
/// the event's id as well.
pub(super) fn of_event(tag: &'static str, graph: GraphId, event: EventPort) -> WidgetId {
    WidgetId::from_hash((
        "darkroom.graph.node",
        graph,
        tag,
        event.node_id,
        event.event,
    ))
}

/// The node's outer body panel, probed by the wire breaker and the geometry rebuild for last
/// frame's arranged rect.
pub(crate) fn body(graph: GraphId, node_id: NodeId) -> WidgetId {
    of_node("body", graph, node_id)
}

/// A node's footer, under its ports. No widget takes a press in the middle of it, past the run
/// status at its left, so a press there grabs the body.
pub(crate) fn footer(graph: GraphId, node_id: NodeId) -> WidgetId {
    of_node("footer", graph, node_id)
}

/// A node's inline title-rename editor and its idle label, so the same id is recorded across
/// the swap. Polled by [`drag_handles`] to drag the node by its title.
pub(crate) fn rename(graph: GraphId, node_id: NodeId) -> WidgetId {
    of_node("title_rename", graph, node_id)
}

pub(crate) fn port_circle(graph: GraphId, port: PortRef) -> WidgetId {
    of_port("port_circle", graph, port)
}

/// A ghost port's hollow dot. An id space of its own, apart from [`port_circle`]: a ghost port
/// is no end a wire drag starts on or snaps to.
pub(super) fn ghost_port(graph: GraphId, port: PortRef) -> WidgetId {
    of_port("ghost_port", graph, port)
}

/// A ghost event's glyph; [`ghost_port`] for an event.
pub(super) fn ghost_event(graph: GraphId, event: EventPort) -> WidgetId {
    of_event("ghost_event", graph, event)
}

/// An input port's inline const editor (text field, checkbox, or file-pick
/// button).
pub(crate) fn const_editor(graph: GraphId, input: InputPort) -> WidgetId {
    of_port("const_editor", graph, input.into())
}

/// An input port's cell (circle and label). Answers a double-click on the label area; the
/// circle has its own [`port_circle`] and consumes hits over its own rect.
pub(super) fn input_cell(graph: GraphId, port: PortRef) -> WidgetId {
    of_port("input_cell", graph, port)
}

/// An event port glyph. A separate id space from [`port_circle`], as events are indexed
/// independently of outputs.
pub(super) fn event_glyph(graph: GraphId, event: EventPort) -> WidgetId {
    of_event("event_glyph", graph, event)
}

/// A trigger's glyph, where a subscription lands: a node's own on its header, or an `Outputs`
/// node's row, so `CanvasGeometry` and `SubscriptionUi` poll its geometry as a wire drop target.
pub(super) fn trigger_glyph(graph: GraphId, trigger: TriggerPort) -> WidgetId {
    WidgetId::from_hash(("trigger_glyph", graph, trigger.node_id, trigger.trigger))
}

/// A ghost trigger's glyph, where a ghost subscription ends on a trigger its node does not show:
/// a node's own on its header, or an `Outputs` node's ghost row. An id space apart from
/// [`trigger_glyph`], so no subscription drag starts on or snaps to it.
pub(crate) fn ghost_trigger_glyph(graph: GraphId, trigger: TriggerPort) -> WidgetId {
    WidgetId::from_hash((
        "ghost_trigger_glyph",
        graph,
        trigger.node_id,
        trigger.trigger,
    ))
}

/// A preview's value area, so the canvas-level scan can read last frame's click.
pub(crate) fn preview_image(graph: GraphId, node_id: NodeId) -> WidgetId {
    of_node("preview_image", graph, node_id)
}

/// A node's inspect button in the header.
pub(super) fn inspect_button(graph: GraphId, node_id: NodeId) -> WidgetId {
    of_node("inspect_button", graph, node_id)
}

/// The hover and press target of `tie`'s band.
pub(super) fn tie_body(graph: GraphId, tie: TieId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.tie", graph, "body", tie))
}

/// The knob of a selected `tie`.
pub(super) fn tie_knob(graph: GraphId, tie: TieId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.tie", graph, "knob", tie))
}

/// Every widget whose drag moves `node_id`'s body.
///
/// Not just the body panel: the header title's idle label senses `DRAG` and swallows the
/// press, so the body never sees one latched there.
///
/// Deliberately curated, not everything under the node. Port circles, header chips, and the
/// inline const editors all latch a drag of their own once the pointer travels (palantir's drag
/// latch ignores `Sense`, so even a `Sense::CLICK` port circle reports one). Each owns its own
/// gesture, so a subtree-wide "did anything start a drag" would wrongly move the node along
/// with them. Palantir's tab chips carry the same shape for the same reason.
pub(super) fn drag_handles(graph: GraphId, node_id: NodeId) -> impl Iterator<Item = WidgetId> {
    [body(graph, node_id), rename(graph, node_id)].into_iter()
}

/// Pointer-over-node for hover-reveal affordances (the const-editor chips). The body
/// response's own `hovered` flag misses most of the node's area, as ports, chips, and editors
/// capture the hit, so this asks whether the hover target sits anywhere in the node's
/// subtree. Target-derived (not a raw `pointer_pos` rect test) on purpose: it changes only
/// when the hover target does, when a repaint is already scheduled, so no `MOVE` subscription
/// is needed, and it is occlusion-aware.
///
/// Resolved against last frame's hover target and cascade, so the answer does not depend on
/// where in this frame's record it is asked; the record pass settles it once per node with
/// [`NodeCtx::with_hover`](crate::gui::graph_ctx::node_ctx::NodeCtx::with_hover) before the
/// subtree that reads it has recorded.
pub(crate) fn hover(ui: &Ui, graph: GraphId, node_id: NodeId) -> Hover {
    Hover::from(ui.is_hover_within(body(graph, node_id)))
}

/// Whether a hovered tie lights `wire`: the pointer is over a tie on its route, as a hovered
/// endpoint lights a wire.
pub(super) fn lit_by_tie(ui: &Ui, graph: GraphId, view: &GraphView, wire: WireKey) -> bool {
    view.routes.get(&wire).is_some_and(|route| {
        route
            .iter()
            .any(|&tie| ui.response_for(tie_body(graph, tie)).hovered())
    })
}

#[cfg(test)]
mod tests {
    use scenarium::{CompoundId, GraphId, NodeId, PortId};

    use crate::core::document::tie::TieId;
    use crate::gui::pane::graph_ui::{canvas, wid};
    use crate::gui::port_ref::PortRef;

    /// A copy of a definition keeps its node ids, so one node id can stand in two panes: each id
    /// a node, a port, a tie or a canvas derives differs between two graphs, and is the same for
    /// one graph asked twice.
    #[test]
    fn one_node_id_in_two_graphs_has_two_sets_of_ids() {
        let (node, tie) = (NodeId::from_u128(7), TieId::from_u128(9));
        let port = PortRef::output(node, PortId::new(1));
        let ids = |graph: GraphId| {
            [
                wid::body(graph, node),
                wid::footer(graph, node),
                wid::rename(graph, node),
                wid::port_circle(graph, port),
                wid::tie_body(graph, tie),
                canvas::outer_wid(graph),
            ]
        };
        let (main, sub) = (GraphId::Main, GraphId::Compound(CompoundId::from_u128(1)));
        assert_eq!(ids(main), ids(main));
        for (in_main, in_sub) in ids(main).into_iter().zip(ids(sub)) {
            assert_ne!(in_main, in_sub);
        }
    }
}
