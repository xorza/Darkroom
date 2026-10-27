use palantir::{PointerButton, PointerEvent, PointerWake, Ui};
use scenarium::Binding;

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::wire::WirePaint;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::frame::wire::Wire;
use crate::gui::pane::graph_ui::gesture::glyph_drag::GlyphDrag;
use crate::gui::pane::graph_ui::gesture::preview_drag;
use crate::gui::port_ref::{PortKind, PortRef};

/// Owns the in-flight new wire — a held drag or a free-floating
/// wire, see [`InFlight`]. Single-wire-at-a-time means one `Option` is
/// enough. The permanent wires aren't state at all: they live on the
/// authoring graph and are painted straight off it by
/// [`WirePass::draw_data_wires`](crate::gui::pane::graph_ui::draw::wire::WirePass::draw_data_wires),
/// which needs nothing from here.
#[derive(Default, Debug)]
pub(crate) struct WireUi {
    state: Option<InFlight>,
}

/// How a wire ended this frame, beyond the edit it raised, which the canvas acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WireEnd {
    /// A held wire released over empty canvas, from this port: the palette opens on it, and
    /// the wire floats from it again once a node is picked (see [`DragMode::Floating`]).
    DroppedOnCanvas(PortRef),
    /// A floating wire ended on a right press, which the canvas spends, so the press reads
    /// purely as "cancel" and its click opens no palette.
    Secondary,
}

/// The in-flight wire being created: a [`GlyphDrag`] within the data-port
/// domain (both ends are `PortRef`s — a wire can be pulled from either kind),
/// plus which terminating input ends it. Both modes share one preview renderer
/// ([`WireUi::draw_in_flight`]), snap tracking, and data, so `mode` is a
/// discriminant rather than distinct payloads.
#[derive(Clone, Copy, Debug)]
pub(super) struct InFlight {
    pub(super) drag: GlyphDrag<PortRef, PortRef>,
    pub(super) mode: DragMode,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DragMode {
    /// LMB-drag from the latched port; ends on button release. The original
    /// gesture: a compatible snap commits, an input dropped on its
    /// own body gets a const, a drop on empty canvas opens the palette.
    Held,
    /// Free wire following the cursor with **no button held** — entered
    /// after a dropped wire spawned a node, so the user aims at the exact
    /// port. A left-click over a compatible port commits; a left-click
    /// elsewhere, a right-click, or Esc cancels.
    Floating,
}

impl WireUi {
    /// Drop the in-flight wire.
    pub(crate) const fn reset(&mut self) {
        self.state = None;
    }

    /// Whether a wire is in flight.
    pub(crate) const fn in_flight(&self) -> bool {
        self.state.is_some()
    }

    /// Re-enter [`DragMode::Floating`] off `start`, the source of a wire
    /// dropped on empty canvas whose palette pick spawned a node, so the user
    /// clicks the exact port to land it. Called in the record that saw the pick.
    ///
    /// `start` was latched frames earlier, so a Ctrl+Z in between can have removed its node.
    /// Not latching is how the wire drops, the same answer `prepass`'s re-latch gives for the same
    /// window.
    pub(crate) fn resume(&mut self, start: PortRef, graph_ctx: GraphCtx<'_>) {
        if graph_ctx.contains(start.node_id) {
            self.state = Some(InFlight {
                drag: GlyphDrag::new(start),
                mode: DragMode::Floating,
            });
        }
    }

    /// Drive the in-flight wire: latch a fresh drag, track the snap
    /// target, and resolve on the active mode's terminating input.
    ///
    /// Latch: the first port whose `PortLayer::first_drag_started` fires starts
    /// a [`DragMode::Held`] wire. While active, every port is rescanned each
    /// frame for the topmost opposite-kind port under the pointer
    /// (`drag.snap`). [`DragMode::Held`] resolves on release; a drop on
    /// empty canvas opens the new-node palette instead of dropping, and
    /// [`Self::resume`] re-enters [`DragMode::Floating`] once a node is
    /// picked. The context's Esc — resolved once by the canvas — drops either
    /// mode without emitting anything. Reports how the wire ended, when that asks the canvas
    /// for more than an edit.
    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        ctx: CanvasCtx<'_>,
        out: &mut Requests,
    ) -> Option<WireEnd> {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        // Latch a fresh port drag only when idle.
        // The output column drops out while the preview-spawn chord is
        // held: that chord is the preview drag's, so the two controllers
        // never both latch one press.
        let preview_chord = preview_drag::modifier_held(ui);
        if self.state.is_none()
            && let Some(drag) = GlyphDrag::latch(&geometry.ports, |port| {
                !preview_chord || port.kind == PortKind::Input
            })
            && graph_ctx.contains(drag.node())
        {
            self.state = Some(InFlight {
                drag,
                mode: DragMode::Held,
            });
        }
        if ctx.cancelled() {
            self.state = None;
        }
        let mut state = self.state.take()?;
        // Both modes span frames, and undo runs before this prepass, so the node the wire grew out
        // of can be deleted under it. Not re-latching is how the gesture drops — a commit against a
        // dead producer is refused at the edit boundary anyway, silently.
        if !graph_ctx.contains(state.drag.node()) {
            return None;
        }

        // Refresh the compatible port under the pointer for both modes.
        state.drag.snap = scan_snap_target(ui, ctx, state.drag.from);
        self.state = Some(state);

        match state.mode {
            DragMode::Held => self.resolve_held(ui, ctx, state.drag, out),
            DragMode::Floating => self.resolve_floating(ui, state.drag, out),
        }
    }

    /// `Held` release: commit a snapped port, else set a const on an
    /// input dropped on its own body, else open the new-node palette for a
    /// drop on empty canvas. While the button is still down, keep the wire.
    fn resolve_held(
        &mut self,
        ui: &mut Ui,
        ctx: CanvasCtx<'_>,
        drag: GlyphDrag<PortRef, PortRef>,
        out: &mut Requests,
    ) -> Option<WireEnd> {
        if drag.held(&ctx.geometry().ports) {
            return None;
        }
        self.state = None;
        if let Some(end) = drag.snap {
            out.push_graph(wire(drag.from, end));
            return None;
        }
        if let Some(set_const) = const_drop(ui, ctx, drag.from) {
            out.push_graph(set_const);
            return None;
        }
        dropped_on_empty_canvas(ui, ctx.geometry()).then_some(WireEnd::DroppedOnCanvas(drag.from))
    }

    /// `Floating` resolve: the wire follows the cursor with no button held,
    /// so its terminating clicks aren't a widget drag — read them off the
    /// global pointer stream (subscribe to wake it; it's empty otherwise).
    /// Left-click lands the wire on a compatible port (or cancels if over
    /// none); right-click cancels (and suppresses the palette); Esc is
    /// handled in `prepass`.
    fn resolve_floating(
        &mut self,
        ui: &mut Ui,
        drag: GlyphDrag<PortRef, PortRef>,
        out: &mut Requests,
    ) -> Option<WireEnd> {
        // `MOVE` wakes a repaint on every cursor move so the wire tracks the
        // pointer (no button is held, so there's no drag-capture keeping
        // frames coming — without this it only redraws when some other
        // widget's hover change happens to wake a frame). `BUTTONS` delivers
        // the terminating press.
        ui.watch_pointer(PointerWake::MOVE | PointerWake::BUTTONS);
        let ended = ui.pointer_events().iter().find_map(|ev| match ev {
            PointerEvent::Down {
                button: button @ (PointerButton::Left | PointerButton::Right),
                ..
            } => Some(*button),
            _ => None,
        });
        match ended {
            Some(PointerButton::Left) => {
                self.state = None;
                out.extend_graph(drag.snap.map(|end| wire(drag.from, end)));
                None
            }
            Some(PointerButton::Right) => {
                self.state = None;
                Some(WireEnd::Secondary)
            }
            _ => None, // keep floating
        }
    }

    /// Force the hover flag on the port the wire is currently snapped to, so
    /// `port_row` picks the hover color up through the same lookup it uses for
    /// an ordinary mouse-over — palantir suppresses `response.hovered` on
    /// every widget but the drag-capture owner while a drag is live, so the
    /// snapped-but-not-captured target would otherwise stay at its idle color.
    pub(crate) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        if let Some(snap) = self.state.as_ref().and_then(|state| state.drag.snap) {
            geometry.ports.set_hovered(snap);
        }
    }

    /// Paint the in-flight drag preview: cubic from the start port's
    /// center to either the snapped target's center (when set) or the
    /// pointer position. Drawn inside the inner canvas so coordinates
    /// share the pan/zoom transform with permanent wires.
    pub(crate) fn draw_in_flight(&self, ui: &mut Ui, ctx: CanvasCtx<'_>) {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        let Some(state) = self.state.as_ref().copied() else {
            return;
        };
        let start_port = state.drag.from;
        let Some(start) = geometry.ports.center(start_port) else {
            return;
        };
        let pointer = canvas::world_pointer(ui, graph_ctx.viewport());
        let Some(end) = state.drag.free_end(pointer, &geometry.ports) else {
            return;
        };
        // Orient handles by kind: outputs grow rightward, inputs grow
        // leftward. Same dx algebra as `draw` so the preview matches
        // the eventual permanent curve exactly when snapped.
        let (p0, p3) = match start_port.kind {
            PortKind::Output => (start, end),
            PortKind::Input => (end, start),
        };
        // Tint the in-flight wire by the dragged port's data type, so the
        // preview already reads as the type being connected.
        let theme = graph_ctx.theme();
        let drag_data_type = graph_ctx.data_type(start_port);
        let color = theme.port_color(drag_data_type);
        WirePaint::from(color).stroke(ui, &Wire::data(p0, p3), theme.stroke_width);
    }
}

/// Whether `port` is a const-only input — one that rejects a wired binding, so a
/// dragged wire must never snap to it or start a bind from it.
fn input_const_only(graph_ctx: GraphCtx<'_>, port: PortRef) -> bool {
    port.input_port()
        .and_then(|input| graph_ctx.node(input.node_id)?.input(input.port_idx))
        .is_some_and(InputCtx::const_only)
}

/// Port currently under the pointer that is a compatible target for `start` —
/// opposite kind, a different node, type-compatible, and not cycle-forming.
/// Uses a geometry test against the cached port rect rather than
/// `response.hovered`: palantir suppresses `hovered` on every widget except the
/// LMB-capture owner during a drag, so while the start port owns the capture no
/// other port can ever read `hovered = true`.
fn scan_snap_target(ui: &mut Ui, ctx: CanvasCtx<'_>, start: PortRef) -> Option<PortRef> {
    let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
    let pointer = ui.pointer_pos()?;
    // A const-only input rejects wired bindings: a drag that starts on one never
    // snaps anywhere, so its release falls through to the set-const gesture.
    if input_const_only(graph_ctx, start) {
        return None;
    }
    let candidates = graph_ctx
        .nodes()
        .filter(|n| n.node_id != start.node_id)
        .flat_map(|n| n.ports(start.kind.opposite()));
    // Geometrically only one port sits under the pointer, so a port the
    // pointer is over but that `accepts_wire` rejects falls through to
    // `None` (drop) rather than snapping elsewhere.
    geometry
        .ports
        .first_containing(pointer, candidates)
        .filter(|&port| accepts_wire(graph_ctx, start, port))
}

/// Whether a wire dragged from `start` may land on `port`: the graph admits
/// the bind, and it closes no data cycle — the two questions the edit gate
/// asks of it, asked of the same authoring graph, so the snap filter and
/// `GraphIntent::into_step` cannot answer differently. Both ends are live
/// graph nodes: the wire drops once its start is gone, and the candidates
/// come from the graph's own nodes.
fn accepts_wire(graph_ctx: GraphCtx<'_>, start: PortRef, port: PortRef) -> bool {
    // `start.kind` fixes which side is the producer (output) and which the
    // consumer (input).
    let (producer, consumer) = match start.kind {
        PortKind::Output => (start, port),
        PortKind::Input => (port, start),
    };
    let input = consumer.input_port().expect("a wire lands on an input");
    let output = producer.output_port().expect("a wire reads an output");
    let graph = graph_ctx.body();
    graph
        .admit(graph_ctx.library(), input, &Binding::Wire(output))
        .is_ok()
        && !graph.produces_cycle(output.node_id, input.node_id)
}

/// "Set const" gesture: an input-port drag released over its own
/// node's body (and not onto a compatible port) means the user
/// wants a literal there. Returns the `SetInput { Const(default) }`
/// intent, or `None` when the gesture doesn't apply — drag started
/// on an output, released off the start node, the port is unknown,
/// or the input is already a const (don't clobber the value).
fn const_drop(ui: &mut Ui, ctx: CanvasCtx<'_>, start: PortRef) -> Option<GraphIntent> {
    let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
    let port = start.input_port()?;
    let pointer = ui.pointer_pos()?;
    if !geometry.node_screen_rect(port.node_id)?.contains(pointer) {
        return None;
    }
    // Don't overwrite an existing const value.
    let input = graph_ctx.node(port.node_id)?.input(port.port_idx)?;
    if matches!(input.binding(), Some(Binding::Const(_))) {
        return None;
    }
    let default = input.default()?;
    Some(GraphIntent::set_input(port, Binding::Const(default)))
}

/// Whether the pointer is over the canvas but not over any node body —
/// the "released into empty space" condition that offers the new-node
/// palette. Both halves are screen-space, the frame the raw pointer is in;
/// the node half comes off `CanvasGeometry`'s snapshot of this frame's body
/// rects, taken in the same sweep `const_drop` reads.
fn dropped_on_empty_canvas(ui: &mut Ui, geometry: &CanvasGeometry) -> bool {
    let Some(pointer) = ui.pointer_pos() else {
        return false;
    };
    let over_canvas = ui
        .response_for(canvas::outer_wid())
        .rect
        .is_some_and(|r| r.contains(pointer));
    over_canvas && !geometry.over_any_node(pointer)
}

/// The `GraphIntent::SetInput` binding a snapped `(start, end)` `PortRef`
/// pair (one `Input`, one `Output` — caller-guaranteed by
/// [`scan_snap_target`]) commits. A cycle-forming pair never reaches here —
/// [`scan_snap_target`] refuses to snap one, and `GraphIntent::into_step`
/// rejects any cycle-forming bind that slips through.
fn wire(start: PortRef, end: PortRef) -> GraphIntent {
    let pair = start
        .input_port()
        .zip(end.output_port())
        .or_else(|| end.input_port().zip(start.output_port()));
    // Both entries into a commit snap through `scan_snap_target`, which only
    // ever offers `start.kind.opposite()`. A same-kind pair means that
    // invariant broke upstream — not that the user did something odd — and
    // silently dropping it would hide the break behind a wire that just
    // doesn't land.
    let Some((input, output)) = pair else {
        unreachable!("a wire committed a {:?} → {:?} pair", start.kind, end.kind);
    };
    GraphIntent::SetInput {
        input,
        to: Some(Binding::Wire(output)),
    }
}

#[cfg(test)]
mod tests;
