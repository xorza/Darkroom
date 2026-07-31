use glam::Vec2;
use palantir::{CurveBrush, PointerButton, PointerEvent, PointerWake, Ui};
use scenarium::DataType;
use scenarium::{Binding, InputPort};

use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::graph::node::port_color::port_color;
use crate::gui::pane::graph::node::set_input;
use crate::gui::pane::graph::paint::wire::{GlyphDrag, Wire, WirePass, WireTint};
use crate::gui::pane::graph::{outer_canvas_widget_id, preview_drag_modifier};
use crate::gui::theme::Theme;

/// Owns the in-flight new-connection wire — a held drag or a free-floating
/// wire, see [`InFlight`]. Single-wire-at-a-time means one `Option` is
/// enough. The permanent connections aren't state at all: they live on the
/// authoring graph and are painted straight off it by the module-level
/// [`draw`], which needs nothing from here.
#[derive(Default, Debug)]
pub(crate) struct ConnectionUI {
    state: GestureSlot<InFlight>,
    /// Source port of a wire dropped on empty canvas this frame. Handed to
    /// the new-node popup so it opens; the wire then resumes *floating*
    /// once a node is picked (see [`DragMode::Floating`]). Taken by the
    /// canvas the same frame.
    pending_open: GestureSlot<PortRef>,
    /// Set when a floating wire ended on a right-click this frame, so the
    /// canvas can suppress the new-node popup that same right-click would
    /// otherwise open — a right-click then reads purely as "cancel".
    ended_on_secondary: bool,
}

/// The in-flight wire being created: a [`GlyphDrag`] within the data-port
/// domain (both ends are `PortRef`s — a wire can be pulled from either kind),
/// plus which terminating input ends it. Both modes share one preview renderer
/// ([`ConnectionUI::draw_in_flight`]), snap tracking, and data, so `mode` is a
/// discriminant rather than distinct payloads.
#[derive(Clone, Copy, Debug)]
pub(super) struct InFlight {
    pub(crate) drag: GlyphDrag<PortRef, PortRef>,
    pub(crate) mode: DragMode,
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

impl ConnectionUI {
    /// Drive the in-flight wire: latch a fresh drag, track the snap
    /// target, and resolve on the active mode's terminating input.
    ///
    /// Latch: the first port whose `PortLayer::first_drag_started` fires starts
    /// a [`DragMode::Held`] wire. While active, every port is rescanned each
    /// frame for the topmost opposite-kind port under the pointer
    /// (`drag.snap`). [`DragMode::Held`] resolves on release; a drop on
    /// empty canvas opens the new-node palette instead of dropping, and
    /// `resume` (the source of such a wire, after its node was picked)
    /// re-enters [`DragMode::Floating`] so the user clicks the exact port
    /// to land it. The context's Esc — resolved once by the canvas — drops
    /// either mode without emitting anything.
    ///
    /// Swept over the whole scene once per frame. The latch scan spans
    /// every pane (only one press exists), but everything after it runs
    /// against the pane that owns the wire's start node — which is also
    /// what makes a cross-pane wire unrepresentable: the snap scan never
    /// sees another graph's ports.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        cx: CanvasCtx<'_>,
        resume: Option<PortRef>,
        out: &mut Intents,
    ) {
        let (graph_ctx, geometry) = (cx.graph_ctx(), cx.geometry());
        self.ended_on_secondary = false;

        // A dropped wire whose palette pick spawned a node resumes floating.
        // `resume` names the wire's *source* port — the node it was dragged
        // from, latched a frame earlier — and the pick lands in the draw phase
        // while `apply_undo_redo` runs ahead of this prepass, so a Ctrl+Z in
        // between can have removed it. Not latching is how the wire drops,
        // the same answer the re-latch below gives for the same window.
        if let Some(start) = resume
            && graph_ctx.contains(start.node_id)
        {
            self.state.latch(InFlight {
                drag: GlyphDrag::new(start),
                mode: DragMode::Floating,
            });
        }
        // Latch a fresh port drag only when idle.
        let candidates = drag_candidates(graph_ctx, preview_drag_modifier(ui));
        if self.state.is_idle()
            && let Some(drag) = GlyphDrag::latch(&geometry.ports, candidates)
            && graph_ctx.contains(drag.node())
        {
            self.state.latch(InFlight {
                drag,
                mode: DragMode::Held,
            });
        }
        if cx.cancelled() {
            self.state.clear();
        }
        let Some(mut state) = self.state.take() else {
            return;
        };
        // Both modes span frames, and undo runs before this prepass, so the
        // pane can close and the node the wire grew out of can be deleted
        // under it. Not re-latching is how the gesture drops — a commit
        // against a dead producer is refused at the edit boundary anyway,
        // silently, and `port_data_type` would meanwhile report the start
        // as untyped (which `scan_snap_target` reads as "compatible with
        // anything").
        if !graph_ctx.contains(state.drag.node()) {
            return;
        }

        // Refresh the compatible port under the pointer for both modes.
        state.drag.snap = scan_snap_target(geometry, ui, graph_ctx, state.drag.from);
        self.state.latch(state);

        match state.mode {
            DragMode::Held => self.resolve_held(ui, graph_ctx, geometry, state.drag, out),
            DragMode::Floating => self.resolve_floating(ui, state.drag, out),
        }
    }

    /// Take the source port of a wire dropped on empty canvas this frame —
    /// the canvas hands it to the new-node popup to open it.
    pub(crate) fn take_pending_connection(&mut self) -> Option<PortRef> {
        self.pending_open.take()
    }

    /// Whether a new-connection gesture is in flight — feeds the wire-fade
    /// tier. (A method, not a `pub(crate)` field: `InFlight` is
    /// module-private.)
    pub(crate) fn is_dragging(&self) -> bool {
        self.state.get().is_some()
    }

    /// Whether a floating wire ended on a right-click this frame — the
    /// canvas suppresses the palette that same right-click would open.
    pub(crate) fn ended_on_secondary(&self) -> bool {
        self.ended_on_secondary
    }

    /// `Held` release: commit a snapped port, else set a const on an
    /// input dropped on its own body, else open the new-node palette for a
    /// drop on empty canvas. While the button is still down, keep the wire.
    fn resolve_held(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        geometry: &CanvasGeometry,
        drag: GlyphDrag<PortRef, PortRef>,
        out: &mut Intents,
    ) {
        if drag.held(&geometry.ports) {
            return;
        }
        if let Some(end) = drag.snap {
            commit_connection(drag.from, end, out);
        } else if let Some(intent) = const_drop(ui, graph_ctx, geometry, drag.from) {
            out.push(intent);
        } else if dropped_on_empty_canvas(ui, geometry) {
            // Open the palette and remember the source; the wire resumes
            // floating once a node is picked.
            self.pending_open.latch(drag.from);
        }
        self.state.clear();
    }

    /// `Floating` resolve: the wire follows the cursor with no button held,
    /// so its terminating clicks aren't a widget drag — read them off the
    /// global pointer stream (subscribe to wake it; it's empty otherwise).
    /// Left-click lands the wire on a compatible port (or cancels if over
    /// none); right-click cancels (and suppresses the palette); Esc is
    /// handled in `apply`.
    fn resolve_floating(
        &mut self,
        ui: &mut Ui,
        drag: GlyphDrag<PortRef, PortRef>,
        out: &mut Intents,
    ) {
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
                if let Some(end) = drag.snap {
                    commit_connection(drag.from, end, out);
                }
                self.state.clear();
            }
            Some(PointerButton::Right) => {
                self.ended_on_secondary = true;
                self.state.clear();
            }
            _ => {} // keep floating
        }
    }

    /// Force the hover flag on the port the wire is currently snapped to, so
    /// `port_row` picks the hover color up through the same lookup it uses for
    /// an ordinary mouse-over — palantir suppresses `response.hovered` on
    /// every widget but the drag-capture owner while a drag is live, so the
    /// snapped-but-not-captured target would otherwise stay at its idle color.
    pub(crate) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        if let Some(snap) = self.state.get().and_then(|state| state.drag.snap) {
            geometry.ports.set_hovered(snap);
        }
    }

    /// Paint the in-flight drag preview: cubic from the start port's
    /// center to either the snapped target's center (when set) or the
    /// pointer position. Drawn inside the inner canvas so coordinates
    /// share the pan/zoom transform with permanent connections.
    pub(crate) fn draw_in_flight(&self, ui: &mut Ui, cx: CanvasCtx<'_>, canvas_origin: Vec2) {
        let (graph_ctx, geometry) = (cx.graph_ctx(), cx.geometry());
        // Scoped: the preview belongs to the pane holding the wire's
        // start node. Unscoped, every *other* pane also drew it — from
        // its own `canvas_origin` and under its own transform, so the
        // wire's graph-space endpoints landed as a phantom curve over an
        // unrelated graph.
        let Some(state) = self.state.get().copied() else {
            return;
        };
        let start_port = state.drag.from;
        let Some(start) = geometry.ports.center(start_port) else {
            return;
        };
        let Some(end) = state
            .drag
            .free_end(ui, graph_ctx, canvas_origin, &geometry.ports)
        else {
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
        let drag_ty = port_data_type(graph_ctx, start_port).unwrap_or_default();
        let color = port_color(theme, &drag_ty, start_port.kind, false);
        Wire::data(p0, p3).add(ui, theme.connection_width, CurveBrush::Solid(color));
    }
}

/// Paint every permanent connection on the current scene retained by the
/// pass's cull region, marking those the active breaker crosses as broken
/// via `probe.mark_broken_input` for the breaker's release-frame drain.
///
/// Reads nothing but committed scene state off `pass`, so it belongs to the
/// module rather than [`ConnectionUI`] — the in-flight gesture that struct
/// owns has no bearing on how the standing wires paint.
pub(crate) fn draw(ui: &mut Ui, pass: &mut WirePass<'_, '_>) {
    let (theme, graph_ctx, geometry) =
        (pass.dcx.theme(), pass.dcx.graph_ctx(), pass.dcx.geometry());
    for (consumer, producer) in graph_ctx.connections() {
        let (src, tgt) = (PortRef::from(producer), PortRef::from(consumer));
        let (Some(p0), Some(p3)) = (geometry.ports.center(src), geometry.ports.center(tgt)) else {
            continue;
        };
        let hover = geometry.ports.is_hovered(src) || geometry.ports.is_hovered(tgt);
        let wire = Wire::data(p0, p3);
        if pass.draw_wire(ui, &wire, hover, || data_tint(theme, graph_ctx, src, tgt)) {
            pass.probe.mark_broken_input(consumer);
        }
    }
}

/// The endpoint colors of a committed data wire: the two ports' type colors,
/// so each end matches the port it touches and the wire reads as its data type
/// (both ends share it unless one side is the untyped `Any` wildcard).
///
/// A wire an upstream wildcard retype left type-mismatched paints entirely in
/// the missing-input warning color instead. Nothing severs it — it lowers as
/// unbound (drift tolerance) — so it wears the same warning the run will report
/// on the port.
fn data_tint(theme: &Theme, graph_ctx: GraphCtx<'_>, src: PortRef, tgt: PortRef) -> WireTint {
    let src_ty = port_data_type(graph_ctx, src).unwrap_or_default();
    let tgt_ty = port_data_type(graph_ctx, tgt).unwrap_or_default();
    if !tgt_ty.compatible_with(&src_ty) {
        return WireTint::flat(theme.colors.exec_missing_glow);
    }
    WireTint::new(
        port_color(theme, &src_ty, PortKind::Output, false),
        port_color(theme, &tgt_ty, PortKind::Input, false),
    )
}

/// Every port a fresh wire drag may latch on, a node's inputs before its
/// outputs so the topmost recorded port wins ties (matches paint order).
///
/// The output column drops out while the preview-spawn chord is held
/// ([`preview_drag_modifier`], passed in as `preview_chord` so the returned
/// iterator doesn't keep `Ui` borrowed): that chord is reserved for the
/// preview-spawn drag (see `preview_drag.rs`), so the two controllers never
/// both latch the same press.
fn drag_candidates(graph_ctx: GraphCtx<'_>, preview_chord: bool) -> impl Iterator<Item = PortRef> {
    let kinds: &'static [PortKind] = if preview_chord {
        &[PortKind::Input]
    } else {
        &[PortKind::Input, PortKind::Output]
    };
    graph_ctx
        .nodes()
        .flat_map(|n| kinds.iter().flat_map(move |&kind| n.ports(kind)))
}

/// Whether `port` is a const-only input — one that rejects a wired binding, so a
/// dragged wire must never snap to it or start a bind from it.
fn input_const_only(graph_ctx: GraphCtx<'_>, port: PortRef) -> bool {
    if port.kind != PortKind::Input {
        return false;
    }
    graph_ctx
        .node(port.node_id)
        .and_then(|n| n.input(port.port_idx))
        .is_some_and(|i| i.const_only())
}

/// Port currently under the pointer that is a compatible target for `start` —
/// opposite kind, a different node, type-compatible, and not cycle-forming.
/// Uses a geometry test against the cached port rect rather than
/// `response.hovered`: palantir suppresses `hovered` on every widget except the
/// LMB-capture owner during a drag, so while the start port owns the capture no
/// other port can ever read `hovered = true`.
fn scan_snap_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    start: PortRef,
) -> Option<PortRef> {
    let pointer = ui.pointer_pos()?;
    // A const-only input rejects wired bindings: a drag that starts on one never
    // snaps anywhere, so its release falls through to the set-const gesture.
    if input_const_only(graph_ctx, start) {
        return None;
    }
    let candidates = graph_ctx
        .nodes()
        .filter(|n| n.id != start.node_id)
        .flat_map(|n| n.ports(start.kind.opposite()))
        // A const-only input is never a valid wire target.
        .filter(|&port| !input_const_only(graph_ctx, port));
    // Geometrically only one port sits under the pointer, so a port the
    // pointer is over but that `accepts_wire` rejects falls through to
    // `None` (drop) rather than snapping elsewhere.
    geometry
        .ports
        .first_containing(pointer, candidates)
        .filter(|&port| accepts_wire(graph_ctx, start, port))
}

/// Whether a wire dragged from `start` may land on `port` — the two
/// rejections that outlive the geometric hit test in [`scan_snap_target`].
fn accepts_wire(graph_ctx: GraphCtx<'_>, start: PortRef, port: PortRef) -> bool {
    let compatible = match (
        port_data_type(graph_ctx, start),
        port_data_type(graph_ctx, port),
    ) {
        (Some(a), Some(b)) => a.compatible_with(&b),
        // Missing type info (port not in the scene this frame) — don't
        // block; let the intent layer decide.
        _ => true,
    };
    // ...and reject a drop that would close a data-flow cycle: the planner
    // rejects a cyclic graph outright (`CycleDetected`) and the intent layer
    // refuses to commit one, so the wire must never latch. Asked of the
    // authoring graph rather than the scene's edge mirror, so the snap filter
    // and `build_step` can't answer differently. `start.kind` fixes which side
    // is the producer (output) and which the consumer (input).
    let (producer, consumer) = match start.kind {
        PortKind::Output => (start.node_id, port.node_id),
        PortKind::Input => (port.node_id, start.node_id),
    };
    compatible && !graph_ctx.body().produces_cycle(producer, consumer)
}

/// "Set const" gesture: an input-port drag released over its own
/// node's body (and not onto a compatible port) means the user
/// wants a literal there. Returns the `SetInput { Const(default) }`
/// intent, or `None` when the gesture doesn't apply — drag started
/// on an output, released off the start node, the port is unknown,
/// or the input is already a const (don't clobber the value).
fn const_drop(
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    geometry: &CanvasGeometry,
    start: PortRef,
) -> Option<GraphIntent> {
    if start.kind != PortKind::Input {
        return None;
    }
    let pointer = ui.pointer_pos()?;
    if !geometry.node_screen_rect(start.node_id)?.contains(pointer) {
        return None;
    }
    // Don't overwrite an existing const value.
    let input = graph_ctx.node(start.node_id)?.input(start.port_idx)?;
    if matches!(input.binding(), Some(Binding::Const(_))) {
        return None;
    }
    let default = input.default()?;
    Some(set_input(start, Binding::Const(default)))
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
        .response_for(outer_canvas_widget_id())
        .rect
        .is_some_and(|r| r.contains(pointer));
    over_canvas && !geometry.over_any_node(pointer)
}

/// The declared [`DataType`] of `port` in the current scene, or `None`
/// if the port isn't present (e.g. mid-rebuild).
fn port_data_type(graph_ctx: GraphCtx<'_>, port: PortRef) -> Option<DataType> {
    let node = graph_ctx.node(port.node_id)?;
    let ty = match port.kind {
        PortKind::Input => node.input(port.port_idx)?.ty().clone(),
        PortKind::Output => node.output(port.port_idx)?.ty().clone(),
    };
    Some(ty)
}

/// Convert a snapped `(start, end)` PortRef pair (one `Input`, one
/// `Output` — caller-guaranteed by [`scan_snap_target`]) into an
/// `GraphIntent::SetInput` binding. A cycle-forming pair never reaches here —
/// [`scan_snap_target`] refuses to snap one, and `build_step` rejects any
/// cycle-forming bind that slips through (the planner is the final backstop,
/// `Error::CycleDetected`). Re-typing a wildcard output (passthrough / reroute)
/// severs nothing downstream: a now-mismatched wire is tolerated, drawn in
/// the warning color, and lowers as unbound.
fn commit_connection(start: PortRef, end: PortRef, out: &mut Intents) {
    let (input, output) = match (start.kind, end.kind) {
        (PortKind::Input, PortKind::Output) => (start, end),
        (PortKind::Output, PortKind::Input) => (end, start),
        // Both entries into a commit snap through `scan_snap_target`, which
        // only ever offers `start.kind.opposite()`. A same-kind pair means
        // that invariant broke upstream — not that the user did something
        // odd — and silently dropping it would hide the break behind a wire
        // that just doesn't land.
        (a, b) => unreachable!("a wire committed a {a:?} → {b:?} pair"),
    };
    out.push(GraphIntent::SetInput {
        input: InputPort::new(input.node_id, input.port_idx),
        to: Some(Binding::bind(output.node_id, output.port_idx)),
    });
}

#[cfg(test)]
mod tests;
