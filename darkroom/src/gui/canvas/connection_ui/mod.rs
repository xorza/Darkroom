use glam::Vec2;
use palantir::{
    Color, CurveBrush, LinearGradient, PointerButton, PointerEvent, PointerWake, Stop, Ui,
};
use scenarium::DataType;
use scenarium::{Binding, InputPort, closes_data_cycle};

use crate::core::document::{BoundarySide, PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::wire::{Wire, WirePass};
use crate::gui::canvas::{free_end, outer_canvas_widget_id};
use crate::gui::node::port_color::port_color;
use crate::gui::node::{node_widget_id, set_input};
use crate::gui::scene::{GraphScene, InputBindingView, Scene};

/// Owns the in-flight new-connection wire — a held drag or a free-floating
/// wire, see [`InFlight`]. Single-wire-at-a-time means one `Option` is
/// enough. The permanent connections aren't state at all: they live on
/// `Scene` and are painted straight off it by the module-level [`draw`],
/// which needs nothing from here.
#[derive(Default, Debug)]
pub(super) struct ConnectionUI {
    state: Option<InFlight>,
    /// Source port of a wire dropped on empty canvas this frame. Handed to
    /// the new-node popup so it opens; the wire then resumes *floating*
    /// once a node is picked (see [`InFlight::Floating`]). Taken by the
    /// canvas the same frame.
    pending_open: Option<PortRef>,
    /// Set when a floating wire ended on a right-click this frame, so the
    /// canvas can suppress the new-node popup that same right-click would
    /// otherwise open — a right-click then reads purely as "cancel".
    ended_on_secondary: bool,
}

/// The in-flight wire being created. Both modes share one preview renderer
/// ([`ConnectionUI::draw_in_flight`]), snap tracking, and data — only the
/// terminating input differs (so `mode` is a discriminant, not distinct
/// payloads). Identity-only — port centers resolve every frame from
/// `CanvasGeometry`, so a wire survives layout changes.
#[derive(Clone, Copy, Debug)]
struct InFlight {
    /// The port the wire started from.
    start: PortRef,
    /// Compatible port currently under the pointer, if any — drives the
    /// preview's snap end and the hover highlight.
    snap_end: Option<PortRef>,
    mode: DragMode,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DragMode {
    /// LMB-drag from `start`; ends on button release. The original
    /// gesture: a compatible `snap_end` commits, an input dropped on its
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
    /// Latch: the first port whose `CanvasGeometry::drag_started` fires starts a
    /// [`InFlight::Held`] wire. While active, every port is rescanned each
    /// frame for the topmost opposite-kind port under the pointer
    /// (`snap_end`). [`InFlight::Held`] resolves on release; a drop on
    /// empty canvas opens the new-node palette instead of dropping, and
    /// `resume` (the source of such a wire, after its node was picked)
    /// re-enters [`InFlight::Floating`] so the user clicks the exact port
    /// to land it. Esc cancels either mode without emitting anything.
    ///
    /// Swept over the whole scene once per frame. The latch scan spans
    /// every pane (only one press exists), but everything after it runs
    /// against the pane that owns the wire's start node — which is also
    /// what makes a cross-pane wire unrepresentable: the snap scan never
    /// sees another graph's ports.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        resume: Option<PortRef>,
        out: &mut Intents,
    ) {
        self.ended_on_secondary = false;

        // A just-spawned node hands its dropped wire back to float.
        if let Some(start) = resume {
            self.state = Some(InFlight {
                start,
                snap_end: None,
                mode: DragMode::Floating,
            });
        }
        // Latch a fresh port drag only when idle.
        if self.state.is_none()
            && let Some(start) = scan_drag_start(geometry, scene, ui)
        {
            self.state = Some(InFlight {
                start,
                snap_end: None,
                mode: DragMode::Held,
            });
        }
        if ui.escape_pressed() {
            self.state = None;
            return;
        }
        // Both modes span frames, and undo runs before this prepass, so the
        // node the wire grew out of can disappear under it (or its pane can
        // close). Drop the gesture rather than let it keep snapping — a
        // commit against a dead producer is refused at the edit boundary
        // anyway, silently, and `port_data_type` would meanwhile report the
        // start as untyped (which `scan_snap_target` reads as "compatible
        // with anything").
        let Some(mut state) = self.state else {
            return;
        };
        let Some(graph) = scene.owner(state.start.node_id) else {
            self.state = None;
            return;
        };

        // Refresh the compatible port under the pointer for both modes.
        state.snap_end = scan_snap_target(geometry, ui, graph, state.start);
        self.state = Some(state);

        match state.mode {
            DragMode::Held => {
                self.resolve_held(ui, graph, geometry, state.start, state.snap_end, out)
            }
            DragMode::Floating => {
                self.resolve_floating(ui, graph, state.start, state.snap_end, out)
            }
        }
    }

    /// Take the source port of a wire dropped on empty canvas this frame,
    /// if it started in `graph`'s pane — the canvas hands it to that pane's
    /// new-node popup to open it. Every pane's draw asks, so the ownership
    /// test and the take are one call: a pane that doesn't own the wire
    /// must not consume it.
    pub(super) fn take_pending_connection_in(&mut self, graph: GraphScene<'_>) -> Option<PortRef> {
        let start = self.pending_open?;
        graph.contains(start.node_id).then(|| {
            self.pending_open = None;
            start
        })
    }

    /// Whether a new-connection gesture is in flight — feeds the shared
    /// wire-fade tier. (A method, not a `pub(super)` field: `InFlight` is
    /// module-private.)
    pub(super) fn dragging(&self) -> bool {
        self.state.is_some()
    }

    /// Whether a floating wire ended on a right-click this frame — the
    /// canvas suppresses the palette that same right-click would open.
    pub(super) fn ended_on_secondary(&self) -> bool {
        self.ended_on_secondary
    }

    /// `Held` release: commit a snapped port, else set a const on an
    /// input dropped on its own body, else open the new-node palette for a
    /// drop on empty canvas. While the button is still down, keep the wire.
    fn resolve_held(
        &mut self,
        ui: &mut Ui,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        start: PortRef,
        snap_end: Option<PortRef>,
        out: &mut Intents,
    ) {
        // `CanvasGeometry::dragging` rolls up `drag_delta().is_some() ||
        // drag_started()`; its transition to `false` is the release edge.
        if geometry.ports.dragging(start) {
            return;
        }
        if let Some(end) = snap_end {
            commit_connection(graph, start, end, out);
        } else if let Some(intent) = self.const_drop(ui, graph, start) {
            out.for_graph(graph.target(), |out| out.push(intent));
        } else if dropped_on_empty_canvas(ui, graph) {
            // Open the palette and remember the source; the wire resumes
            // floating once a node is picked.
            self.pending_open = Some(start);
        }
        self.state = None;
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
        graph: GraphScene<'_>,
        start: PortRef,
        snap_end: Option<PortRef>,
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
                if let Some(end) = snap_end {
                    commit_connection(graph, start, end, out);
                }
                self.state = None;
            }
            Some(PointerButton::Right) => {
                self.ended_on_secondary = true;
                self.state = None;
            }
            _ => {} // keep floating
        }
    }

    /// "Set const" gesture: an input-port drag released over its own
    /// node's body (and not onto a compatible port) means the user
    /// wants a literal there. Returns the `SetInput { Const(default) }`
    /// intent, or `None` when the gesture doesn't apply — drag started
    /// on an output, released off the start node, the port is unknown,
    /// or the input is already a const (don't clobber the value).
    fn const_drop(&self, ui: &mut Ui, graph: GraphScene<'_>, start: PortRef) -> Option<Intent> {
        if start.kind != PortKind::Input {
            return None;
        }
        let pointer = ui.pointer_pos()?;
        let body = ui.response_for(node_widget_id(start.node_id)).rect?;
        if !body.contains(pointer) {
            return None;
        }
        let node = graph.node(start.node_id)?;
        // Boundary ports route the interface, not literal values.
        if node.boundary {
            return None;
        }
        // Don't overwrite an existing const value.
        let input = graph.inputs(node.inputs).get(start.port_idx)?;
        if matches!(input.binding, InputBindingView::Const(_)) {
            return None;
        }
        let default = input.default.clone()?;
        Some(set_input(start, Binding::Const(default)))
    }

    /// Compatible-kind port currently snapped under the pointer
    /// during an active drag, or `None`. Read by `GraphUI` to force
    /// the hover state in `CanvasGeometry` (otherwise palantir's
    /// drag-capture suppression would hide it).
    pub(super) fn snap_port(&self) -> Option<PortRef> {
        self.state.and_then(|s| s.snap_end)
    }

    /// Paint the in-flight drag preview: cubic from the start port's
    /// center to either the snapped target's center (when set) or the
    /// pointer position. Drawn inside the inner canvas so coordinates
    /// share the pan/zoom transform with permanent connections.
    pub(super) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        canvas_origin: Vec2,
    ) {
        let Some(state) = self.state else { return };
        let start_port = state.start;
        let Some(start) = geometry.ports.center(start_port) else {
            return;
        };
        let Some(end) = free_end(ui, graph, canvas_origin, &geometry.ports, state.snap_end) else {
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
        let drag_ty = port_data_type(graph, start_port).unwrap_or_default();
        let color = port_color(ctx.theme, &drag_ty, start_port.kind, false);
        Wire::data(p0, p3).add(ui, ctx.theme.connection_width, port_gradient(color, color));
    }
}

/// Paint every permanent connection on the current scene retained by the
/// pass's cull region, marking those the active breaker crosses as broken
/// via `probe.mark_broken_input` for the breaker's release-frame drain.
///
/// Reads nothing but committed scene state off `pass`, so it belongs to the
/// module rather than [`ConnectionUI`] — the in-flight gesture that struct
/// owns has no bearing on how the standing wires paint.
pub(super) fn draw(ui: &mut Ui, pass: &mut WirePass<'_, '_>) {
    let (theme, graph) = (pass.theme, pass.graph);
    for c in graph.connections() {
        let src_port = PortRef {
            node_id: c.src.node_id,
            kind: PortKind::Output,
            port_idx: c.src.port_idx,
        };
        let tgt_port = PortRef {
            node_id: c.tgt.node_id,
            kind: PortKind::Input,
            port_idx: c.tgt.port_idx,
        };
        let (Some(p0), Some(p3)) = (
            pass.geometry.ports.center(src_port),
            pass.geometry.ports.center(tgt_port),
        ) else {
            continue;
        };
        let wire = Wire::data(p0, p3);
        let endpoint_hover =
            pass.geometry.ports.is_hovered(src_port) || pass.geometry.ports.is_hovered(tgt_port);
        let Some(stroke) = pass.resolve(&wire, endpoint_hover) else {
            continue;
        };
        if stroke.broken {
            pass.probe.mark_broken_input(c.tgt);
        }
        // Gradient from output (p0) → input (p3) port color so each
        // end of a connection visually matches the port it touches —
        // and, with per-type port colors, the wire reads as its data
        // type (both ends share it unless one side is the untyped
        // `Any` wildcard). Palantir's cubic-curve lowering samples
        // `CurveBrush::Linear` along the curve parameter `t` and ignores
        // `angle` — we pass 0.0. Broken-state still wins as a flat color
        // so the alarm read doesn't get diluted by the gradient.
        let src_ty = port_data_type(graph, src_port).unwrap_or_default();
        let tgt_ty = port_data_type(graph, tgt_port).unwrap_or_default();
        let brush = if stroke.broken {
            CurveBrush::Solid(theme.colors.connection_broken)
        } else if !tgt_ty.compatible_with(&src_ty) {
            // A wildcard retype upstream left this wire type-mismatched.
            // Nothing severs it — it flattens as unbound (drift
            // tolerance) — so paint it in the missing-input warning
            // color, matching the port glow the run will report.
            CurveBrush::Solid(
                pass.emphasis
                    .tint(theme.colors.exec_missing_glow, stroke.hovered),
            )
        } else {
            let a = pass.emphasis.tint(
                port_color(theme, &src_ty, PortKind::Output, false),
                stroke.hovered,
            );
            let b = pass.emphasis.tint(
                port_color(theme, &tgt_ty, PortKind::Input, false),
                stroke.hovered,
            );
            port_gradient(a, b)
        };
        wire.add(ui, stroke.width, brush);
    }
}

/// Linear gradient running along the curve parameter from `start`
/// (`t = 0`, the output-port side at `p0`) to `end` (`t = 1`, the
/// input-port side at `p3`). Palantir's cubic-curve lowering samples
/// the brush along `t` and ignores `angle`, so the geometric direction
/// doesn't matter here.
fn port_gradient(start: Color, end: Color) -> CurveBrush {
    CurveBrush::Linear(LinearGradient::new(
        0.0,
        [Stop::new(0.0, start), Stop::new(1.0, end)],
    ))
}

/// First port whose response shows `drag_started` this frame, or `None`.
/// Iterates inputs first then outputs per node so the topmost recorded
/// port wins ties (matches paint order). Skips output ports while Cmd is
/// held — that chord is reserved for `PinUi`'s pin-creation drag (see
/// `pin_ui.rs`), so the two controllers never both latch the same press.
fn scan_drag_start(geometry: &CanvasGeometry, scene: &Scene, ui: &mut Ui) -> Option<PortRef> {
    // Cmd is reserved for `PinUi`'s pin-creation drag off an output, so while
    // it's held only the input column is a candidate.
    let kinds: &[PortKind] = if ui.modifiers().ctrl {
        &[PortKind::Input]
    } else {
        &[PortKind::Input, PortKind::Output]
    };
    let keys = scene
        .nodes
        .values()
        .flat_map(|n| kinds.iter().flat_map(move |&kind| n.ports(kind)));
    geometry.ports.first_drag_started(keys)
}

/// Whether `port` is a const-only input — one that rejects a wired binding, so a
/// dragged wire must never snap to it or start a bind from it.
fn input_const_only(graph: GraphScene<'_>, port: PortRef) -> bool {
    if port.kind != PortKind::Input {
        return false;
    }
    graph
        .node(port.node_id)
        .and_then(|n| graph.inputs(n.inputs).get(port.port_idx))
        .is_some_and(|i| i.const_only)
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
    graph: GraphScene<'_>,
    start: PortRef,
) -> Option<PortRef> {
    let pointer = ui.pointer_pos()?;
    // A const-only input rejects wired bindings: a drag that starts on one never
    // snaps anywhere, so its release falls through to the set-const gesture.
    if input_const_only(graph, start) {
        return None;
    }
    // A passthrough (graph input boundary wired straight to the output
    // boundary) leaves the relayed value untyped at execution and panics
    // the worker — disallow it by never snapping one boundary node onto
    // the other. The only boundary→boundary link possible is exactly that
    // passthrough, so a blanket reject is precise.
    let start_boundary = graph.node(start.node_id).is_some_and(|n| n.boundary);
    let candidates = graph
        .nodes()
        .filter(|n| n.id != start.node_id && !(start_boundary && n.boundary))
        .flat_map(|n| n.ports(start.kind.opposite()))
        // A const-only input is never a valid wire target.
        .filter(|&port| !input_const_only(graph, port));
    // Geometrically only one port sits under the pointer, so a port the
    // pointer is over but that `accepts_wire` rejects falls through to
    // `None` (drop) rather than snapping elsewhere.
    geometry
        .ports
        .first_containing(pointer, candidates)
        .filter(|&port| accepts_wire(graph, start, port))
}

/// Whether a wire dragged from `start` may land on `port` — the two
/// rejections that outlive the geometric hit test in [`scan_snap_target`].
fn accepts_wire(graph: GraphScene<'_>, start: PortRef, port: PortRef) -> bool {
    let compatible = match (port_data_type(graph, start), port_data_type(graph, port)) {
        (Some(a), Some(b)) => a.compatible_with(&b),
        // Missing type info (port not in the scene this frame) — don't
        // block; let the intent layer decide.
        _ => true,
    };
    // ...and reject a drop that would close a data-flow cycle: the planner
    // rejects a cyclic graph outright (`CycleDetected`) and the intent layer
    // refuses to commit one, so the wire must never latch. `start.kind` fixes
    // which side is the producer (output) and which the consumer (input).
    // The pane's connection slice is that graph's edge mirror, fed to the
    // same scenarium check the intent layer uses.
    let (producer, consumer) = match start.kind {
        PortKind::Output => (start.node_id, port.node_id),
        PortKind::Input => (port.node_id, start.node_id),
    };
    let edges = graph
        .connections()
        .iter()
        .map(|c| (c.src.node_id, c.tgt.node_id));
    compatible && !closes_data_cycle(edges, producer, consumer)
}

/// Whether the pointer is over the canvas but not over any node body —
/// the "released into empty space" condition that offers the new-node
/// palette. Uses the same arranged-rect hit test as `const_drop`.
fn dropped_on_empty_canvas(ui: &mut Ui, graph: GraphScene<'_>) -> bool {
    let Some(pointer) = ui.pointer_pos() else {
        return false;
    };
    let over_canvas = ui
        .response_for(outer_canvas_widget_id(graph.target()))
        .rect
        .is_some_and(|r| r.contains(pointer));
    over_canvas
        && !graph.nodes().any(|n| {
            ui.response_for(node_widget_id(n.id))
                .rect
                .is_some_and(|r| r.contains(pointer))
        })
}

/// The declared [`DataType`] of `port` in the current scene, or `None`
/// if the port isn't present (e.g. mid-rebuild).
fn port_data_type(graph: GraphScene<'_>, port: PortRef) -> Option<DataType> {
    let node = graph.node(port.node_id)?;
    let ty = match port.kind {
        PortKind::Input => graph.inputs(node.inputs).get(port.port_idx)?.ty.clone(),
        PortKind::Output => graph.outputs(node.outputs).get(port.port_idx)?.ty.clone(),
    };
    Some(ty)
}

/// Convert a snapped `(start, end)` PortRef pair (one `Input`, one
/// `Output` — caller-guaranteed by [`scan_snap_target`]) into an
/// `Intent::SetInput` binding. A cycle-forming pair never reaches here —
/// [`scan_snap_target`] refuses to snap one, and `build_step` rejects any
/// cycle-forming bind that slips through (the planner is the final backstop,
/// `Error::CycleDetected`). Re-typing a wildcard output (passthrough / reroute)
/// severs nothing downstream: a now-mismatched wire is tolerated, drawn in
/// the warning color, and flattens as unbound.
///
/// An endpoint that is a boundary node's trailing "+" placeholder first
/// materializes the interface port it stands for (`Intent::AddBoundaryPort`)
/// in the same batch, so add + bind undo as one entry.
fn commit_connection(graph: GraphScene<'_>, start: PortRef, end: PortRef, out: &mut Intents) {
    let (input, output) = match (start.kind, end.kind) {
        (PortKind::Input, PortKind::Output) => (start, end),
        (PortKind::Output, PortKind::Input) => (end, start),
        _ => return, // unreachable — scan_snap_target enforces opposite kinds
    };
    out.for_graph(graph.target(), |out| {
        out.extend(add_boundary_port_intent(graph, output, input));
        out.extend(add_boundary_port_intent(graph, input, output));
        out.push(Intent::SetInput {
            input: InputPort::new(input.node_id, input.port_idx),
            to: Some(Binding::bind(output.node_id, output.port_idx)),
        });
    });
}

/// When `port` is a boundary node's trailing placeholder, the
/// `Intent::AddBoundaryPort` that materializes its interface slot. Named
/// `input{N}`/`output{N}` (first free N over the node's existing ports)
/// and typed from `opposite` — the placeholder itself is untyped; the
/// other endpoint carries the type (a consumer's declared input type, a
/// producer's resolved output type). `None` for ordinary ports.
fn add_boundary_port_intent(
    graph: GraphScene<'_>,
    port: PortRef,
    opposite: PortRef,
) -> Option<Intent> {
    let node = graph.node(port.node_id)?;
    if !node.boundary {
        return None;
    }
    // A boundary node mirrors the interface: the `GraphInput` node's
    // *output* column is the graph's `Input` side and vice versa.
    let (count, side, prefix) = match port.kind {
        PortKind::Output => (
            graph.outputs(node.outputs).len(),
            BoundarySide::Input,
            "input",
        ),
        PortKind::Input => (
            graph.inputs(node.inputs).len(),
            BoundarySide::Output,
            "output",
        ),
    };
    if port.port_idx + 1 != count {
        return None; // an existing interface port, not the trailing placeholder
    }
    let taken: Vec<String> = match port.kind {
        PortKind::Output => graph.outputs(node.outputs)[..port.port_idx]
            .iter()
            .map(|output| String::from(&*output.name.borrow_str()))
            .collect(),
        PortKind::Input => graph.inputs(node.inputs)[..port.port_idx]
            .iter()
            .map(|input| String::from(&*input.name.borrow_str()))
            .collect(),
    };
    let name = (0..)
        .map(|n| format!("{prefix}{n}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap();
    Some(Intent::AddBoundaryPort {
        side,
        name,
        data_type: port_data_type(graph, opposite).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests;
