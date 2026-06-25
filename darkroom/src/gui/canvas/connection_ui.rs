use glam::Vec2;
use palantir::{
    Brush, Color, LineCap, LinearGradient, PointerButton, PointerEvent, PointerSense, Shape, Stop,
    Ui,
};
use scenarium::data::DataType;
use scenarium::graph::{Binding, InputPort};

use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::port_frame::PortFrame;
use crate::gui::canvas::{node_ports, outer_canvas_widget_id, to_world};
use crate::gui::node::port_color::port_color;
use crate::gui::node::{node_widget_id, set_input};
use crate::gui::scene::{InputBindingView, Scene};
use crate::gui::{PortKind, PortRef};

/// Minimum horizontal length of a connection's bezier control handles,
/// so short/backward links still bow out into a readable S-curve.
const MIN_CUBIC_HANDLE: f32 = 40.0;

/// The two interior control points of a connection cubic.
struct CubicHandles {
    p1: Vec2,
    p2: Vec2,
}

/// Control points for a left-to-right cubic between port centers `p0`
/// (output side) and `p3` (input side). Shared by the permanent and
/// in-flight draws so the preview matches the committed curve exactly.
fn cubic_handles(p0: Vec2, p3: Vec2) -> CubicHandles {
    let dx = ((p3.x - p0.x).abs() * 0.5).max(MIN_CUBIC_HANDLE);
    CubicHandles {
        p1: p0 + Vec2::new(dx, 0.0),
        p2: p3 - Vec2::new(dx, 0.0),
    }
}

/// Owns the in-flight new-connection wire (a held drag or a free-floating
/// wire — see [`InFlight`]) plus the existing-connection renderer.
/// Single-wire-at-a-time means one `Option` is enough; the permanent
/// connection list lives on `Scene` and is iterated each frame by
/// [`Self::draw`].
#[derive(Default, Debug)]
pub(crate) struct ConnectionUI {
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
/// `PortFrame`, so a wire survives layout changes.
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
    /// Latch: the first port whose `PortFrame::drag_started` fires starts a
    /// [`InFlight::Held`] wire. While active, every port is rescanned each
    /// frame for the topmost opposite-kind port under the pointer
    /// (`snap_end`). [`InFlight::Held`] resolves on release; a drop on
    /// empty canvas opens the new-node palette instead of dropping, and
    /// `resume` (the source of such a wire, after its node was picked)
    /// re-enters [`InFlight::Floating`] so the user clicks the exact port
    /// to land it. Esc cancels either mode without emitting anything.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        port_frame: &PortFrame,
        resume: Option<PortRef>,
        out: &mut Vec<Intent>,
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
            && let Some(start) = scan_drag_start(port_frame, scene)
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
        let Some(mut state) = self.state else {
            return;
        };

        // Refresh the compatible port under the pointer for both modes.
        state.snap_end = scan_snap_target(port_frame, ui, scene, state.start);
        self.state = Some(state);

        match state.mode {
            DragMode::Held => {
                self.resolve_held(ui, scene, port_frame, state.start, state.snap_end, out)
            }
            DragMode::Floating => self.resolve_floating(ui, state.start, state.snap_end, out),
        }
    }

    /// Take the source port of a wire dropped on empty canvas this frame,
    /// if any. The canvas hands it to the new-node popup to open it.
    pub(crate) fn take_pending_connection(&mut self) -> Option<PortRef> {
        self.pending_open.take()
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
        ui: &Ui,
        scene: &Scene,
        port_frame: &PortFrame,
        start: PortRef,
        snap_end: Option<PortRef>,
        out: &mut Vec<Intent>,
    ) {
        // `PortFrame::dragging` rolls up `drag_delta().is_some() ||
        // drag_started()`; its transition to `false` is the release edge.
        if port_frame.dragging(start) {
            return;
        }
        if let Some(end) = snap_end {
            commit_connection(start, end, out);
        } else if let Some(intent) = self.const_drop(ui, scene, start) {
            out.push(intent);
        } else if dropped_on_empty_canvas(ui, scene) {
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
        start: PortRef,
        snap_end: Option<PortRef>,
        out: &mut Vec<Intent>,
    ) {
        // `MOVE` wakes a repaint on every cursor move so the wire tracks the
        // pointer (no button is held, so there's no drag-capture keeping
        // frames coming — without this it only redraws when some other
        // widget's hover change happens to wake a frame). `BUTTONS` delivers
        // the terminating press.
        ui.subscribe_pointer(PointerSense::MOVE | PointerSense::BUTTONS);
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
                    commit_connection(start, end, out);
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
    fn const_drop(&self, ui: &Ui, scene: &Scene, start: PortRef) -> Option<Intent> {
        if start.kind != PortKind::Input {
            return None;
        }
        let pointer = ui.pointer_pos()?;
        let body = ui.response_for(node_widget_id(start.node_id)).rect?;
        if !body.contains(pointer) {
            return None;
        }
        let node = scene.nodes.by_key(&start.node_id)?;
        // Boundary ports route the interface, not literal values.
        if node.boundary {
            return None;
        }
        // Don't overwrite an existing const value.
        let input = scene.inputs(node.inputs).get(start.port_idx)?;
        if matches!(input.binding, InputBindingView::Const(_)) {
            return None;
        }
        let default = input.default.clone()?;
        Some(set_input(start, Binding::Const(default)))
    }

    /// Compatible-kind port currently snapped under the pointer
    /// during an active drag, or `None`. Read by `GraphUI` to force
    /// the hover state in `PortFrame` (otherwise palantir's
    /// drag-capture suppression would hide it).
    pub(crate) fn snap_port(&self) -> Option<PortRef> {
        self.state.and_then(|s| s.snap_end)
    }

    /// Paint every permanent connection on the current scene, marking
    /// those the active breaker (`probe.state`) crosses as broken.
    /// Hits get pushed onto `probe.state.broken` for the breaker's
    /// release-frame drain.
    pub(crate) fn draw(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        probe: &mut BreakerProbe<'_>,
    ) {
        let width = ctx.theme.connection_width;
        if let Some(b) = probe.state.as_deref_mut() {
            b.broken.clear();
        }
        for c in &scene.connections {
            let src_port = PortRef {
                node_id: c.src_node,
                kind: PortKind::Output,
                port_idx: c.src_port,
            };
            let tgt_port = PortRef {
                node_id: c.tgt_node,
                kind: PortKind::Input,
                port_idx: c.tgt_port,
            };
            let (Some(p0), Some(p3)) = (
                port_frame.center_canvas_local(src_port),
                port_frame.center_canvas_local(tgt_port),
            ) else {
                continue;
            };
            let CubicHandles { p1, p2 } = cubic_handles(p0, p3);
            let broken = probe
                .state
                .as_deref()
                .is_some_and(|b| b.intersects_cubic(p0, p1, p2, p3));
            if broken {
                // unwrap: `broken == true` implies `state` is `Some`.
                probe
                    .state
                    .as_deref_mut()
                    .unwrap()
                    .broken
                    .push(InputPort::new(c.tgt_node, c.tgt_port));
            }
            // Gradient from output (p0) → input (p3) port color so each
            // end of a connection visually matches the port it touches —
            // and, with per-type port colors, the wire reads as its data
            // type (both ends share it unless one side is the untyped
            // `Null` wildcard). `lower_cubic_bezier` samples `Brush::Linear`
            // along the curve parameter `t` and ignores `angle` — we pass
            // 0.0. Broken-state still wins as a flat color so the alarm
            // read doesn't get diluted by the gradient.
            let brush = if broken {
                Brush::Solid(ctx.theme.connection_broken)
            } else {
                let src_ty = port_data_type(scene, src_port).unwrap_or_default();
                let tgt_ty = port_data_type(scene, tgt_port).unwrap_or_default();
                port_gradient(
                    port_color(ctx.theme, &src_ty, PortKind::Output, false),
                    port_color(ctx.theme, &tgt_ty, PortKind::Input, false),
                )
            };
            ui.add_shape(Shape::CubicBezier {
                p0,
                p1,
                p2,
                p3,
                width,
                brush,
                cap: LineCap::Round,
            });
        }
    }

    /// Paint the in-flight drag preview: cubic from the start port's
    /// center to either the snapped target's center (when set) or the
    /// pointer position. Drawn inside the inner canvas so coordinates
    /// share the pan/zoom transform with permanent connections.
    pub(crate) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        canvas_origin: Vec2,
    ) {
        let Some(state) = self.state else { return };
        let start_port = state.start;
        let Some(start) = port_frame.center_canvas_local(start_port) else {
            return;
        };
        let end = match state.snap_end {
            Some(snap) => port_frame.center_canvas_local(snap),
            None => ui.pointer_pos().map(|p| to_world(p - canvas_origin, scene)),
        };
        let Some(end) = end else { return };
        // Orient handles by kind: outputs grow rightward, inputs grow
        // leftward. Same dx algebra as `draw` so the preview matches
        // the eventual permanent curve exactly when snapped.
        let (p0, p3) = match start_port.kind {
            PortKind::Output => (start, end),
            PortKind::Input => (end, start),
        };
        let CubicHandles { p1, p2 } = cubic_handles(p0, p3);
        // Tint the in-flight wire by the dragged port's data type, so the
        // preview already reads as the type being connected.
        let drag_ty = port_data_type(scene, start_port).unwrap_or_default();
        let wire = port_color(ctx.theme, &drag_ty, start_port.kind, false);
        ui.add_shape(Shape::CubicBezier {
            p0,
            p1,
            p2,
            p3,
            width: ctx.theme.connection_width,
            brush: port_gradient(wire, wire),
            cap: LineCap::Round,
        });
    }
}

/// Linear gradient running along the curve parameter from `start`
/// (`t = 0`, the output-port side at `p0`) to `end` (`t = 1`, the
/// input-port side at `p3`). `lower_cubic_bezier` samples the brush
/// along `t` and ignores `angle`, so the geometric direction doesn't
/// matter here.
fn port_gradient(start: Color, end: Color) -> Brush {
    Brush::Linear(LinearGradient::new(
        0.0,
        [Stop::new(0.0, start), Stop::new(1.0, end)],
    ))
}

/// First port whose response shows `drag_started` this frame, or `None`.
/// Iterates inputs first then outputs per node so the topmost recorded
/// port wins ties (matches paint order).
fn scan_drag_start(frame: &PortFrame, scene: &Scene) -> Option<PortRef> {
    for n in &scene.nodes {
        for kind in [PortKind::Input, PortKind::Output] {
            for port in node_ports(n, kind) {
                if frame.drag_started(port) {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// Port currently under the pointer that is a compatible target for
/// `start` — opposite kind and a different node. Uses a geometry test
/// against the cached `screen_rect` rather than `response.hovered`:
/// palantir suppresses `hovered` on every widget except the
/// LMB-capture owner during a drag, so while the start port owns the
/// capture no other port can ever read `hovered = true`.
/// Whether `port` is a const-only input — one that rejects a wired binding, so a
/// dragged wire must never snap to it or start a bind from it.
fn input_const_only(scene: &Scene, port: PortRef) -> bool {
    if port.kind != PortKind::Input {
        return false;
    }
    scene
        .nodes
        .by_key(&port.node_id)
        .and_then(|n| scene.inputs(n.inputs).get(port.port_idx))
        .is_some_and(|i| i.const_only)
}

fn scan_snap_target(frame: &PortFrame, ui: &Ui, scene: &Scene, start: PortRef) -> Option<PortRef> {
    let want_kind = start.kind.opposite();
    let pointer = ui.pointer_pos()?;
    // A const-only input rejects wired bindings: a drag that starts on one never
    // snaps anywhere, so its release falls through to the set-const gesture.
    if input_const_only(scene, start) {
        return None;
    }
    // A passthrough (subgraph input boundary wired straight to the output
    // boundary) leaves the relayed value untyped at execution and panics
    // the worker — disallow it by never snapping one boundary node onto
    // the other. The only boundary→boundary link possible is exactly that
    // passthrough, so a blanket reject is precise.
    let start_boundary = scene
        .nodes
        .by_key(&start.node_id)
        .is_some_and(|n| n.boundary);
    let start_type = port_data_type(scene, start);
    for n in &scene.nodes {
        if n.id == start.node_id {
            continue;
        }
        if start_boundary && n.boundary {
            continue;
        }
        for port in node_ports(n, want_kind) {
            // A const-only input is never a valid wire target.
            if input_const_only(scene, port) {
                continue;
            }
            if frame.contains_pointer(port, pointer) {
                // Reject a drop onto an incompatible port so the wire
                // won't latch. Geometrically only one port sits under the
                // pointer, so a reject here falls through to `None` (drop)
                // rather than snapping elsewhere.
                let compatible = match (&start_type, port_data_type(scene, port)) {
                    (Some(a), Some(b)) => a.compatible_with(&b),
                    // Missing type info (port not in the scene this frame)
                    // — don't block; let the intent layer decide.
                    _ => true,
                };
                if compatible {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// Whether the pointer is over the canvas but not over any node body —
/// the "released into empty space" condition that offers the new-node
/// palette. Uses the same arranged-rect hit test as `const_drop`.
fn dropped_on_empty_canvas(ui: &Ui, scene: &Scene) -> bool {
    let Some(pointer) = ui.pointer_pos() else {
        return false;
    };
    let over_canvas = ui
        .response_for(outer_canvas_widget_id())
        .rect
        .is_some_and(|r| r.contains(pointer));
    over_canvas
        && !scene.nodes.iter().any(|n| {
            ui.response_for(node_widget_id(n.id))
                .rect
                .is_some_and(|r| r.contains(pointer))
        })
}

/// The declared [`DataType`] of `port` in the current scene, or `None`
/// if the port isn't present (e.g. mid-rebuild).
fn port_data_type(scene: &Scene, port: PortRef) -> Option<DataType> {
    let node = scene.nodes.iter().find(|n| n.id == port.node_id)?;
    let ty = match port.kind {
        PortKind::Input => scene.inputs(node.inputs).get(port.port_idx)?.ty.clone(),
        PortKind::Output => scene.outputs(node.outputs).get(port.port_idx)?.ty.clone(),
    };
    Some(ty)
}

/// Convert a snapped `(start, end)` PortRef pair (one `Input`, one
/// `Output` — caller-guaranteed by [`scan_snap_target`]) into an
/// `Intent::SetInput` binding. Cycle prevention is left to the intent
/// apply layer; an invalid binding will surface there rather than
/// silently failing here.
fn commit_connection(start: PortRef, end: PortRef, out: &mut Vec<Intent>) {
    let (input, output) = match (start.kind, end.kind) {
        (PortKind::Input, PortKind::Output) => (start, end),
        (PortKind::Output, PortKind::Input) => (end, start),
        _ => return, // unreachable — scan_snap_target enforces opposite kinds
    };
    out.push(Intent::SetInput {
        node_id: input.node_id,
        input_idx: input.port_idx,
        to: Binding::bind(output.node_id, output.port_idx),
    });
}
