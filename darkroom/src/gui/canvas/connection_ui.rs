use glam::Vec2;
use palantir::{Brush, Color, LineCap, LinearGradient, Shape, Stop, Ui};
use scenarium::data::DataType;
use scenarium::graph::{Binding, InputPort, OutputPort};

use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::port_frame::PortFrame;
use crate::gui::canvas::{node_ports, to_world};
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

/// Owns the in-flight new-connection drag plus the existing-connection
/// renderer. Single-port-at-a-time means one `Option` is enough; the
/// permanent connection list lives on `Scene` and is iterated each
/// frame by [`Self::draw`]. Mirrors the deprecated
/// `Gesture::DraggingConnection` but as a dedicated UI module.
#[derive(Default, Debug)]
pub(crate) struct ConnectionUI {
    drag: Option<ConnectionDrag>,
}

/// In-flight connection drag. Identity-only — port centers are
/// resolved every frame from `PortFrame`, so the drag survives layout
/// changes without stale coordinates.
#[derive(Clone, Copy, Debug)]
struct ConnectionDrag {
    start: PortRef,
    /// Compatible-kind port currently under the pointer, if any. Set
    /// during the per-frame update and read at release time.
    snap_end: Option<PortRef>,
}

impl ConnectionUI {
    /// Drive the gesture: latch start, track snap target, commit on
    /// release.
    ///
    /// Latch: on the first port whose `PortFrame::drag_started`
    /// fires, store a [`ConnectionDrag`] pinned to that port.
    /// While active: rescan every port each frame for the topmost
    /// opposite-kind port under the pointer; that becomes `snap_end`.
    /// Release resolves to one of three outcomes:
    /// - a compatible `snap_end` → bind the input to the output
    ///   ([`Intent::SetInput`] with `Binding::Bind`);
    /// - dragged from an **input** and dropped back on its **own
    ///   node's body** (no snap) → give that input a default const
    ///   value (quick "I want a literal here" gesture);
    /// - anything else → drop silently.
    ///
    /// Esc cancels without emitting anything.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        port_frame: &PortFrame,
        out: &mut Vec<Intent>,
    ) {
        if self.drag.is_none() {
            self.drag = scan_drag_start(port_frame, scene);
        }
        if ui.escape_pressed() {
            self.drag = None;
            return;
        }
        let Some(mut drag) = self.drag else {
            return;
        };
        // `PortFrame::dragging` rolls up `drag_delta().is_some() || drag_started()`
        // — `Some` (incl. zero-delta first-frame) means live, transition
        // to `false` is the release edge.
        drag.snap_end = scan_snap_target(port_frame, ui, scene, drag.start);
        if port_frame.dragging(drag.start) {
            self.drag = Some(drag);
            return;
        }
        if let Some(end) = drag.snap_end {
            commit_connection(drag.start, end, out);
        } else if let Some(intent) = self.const_drop(ui, scene, drag.start) {
            out.push(intent);
        }
        self.drag = None;
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
        let node = scene.nodes.iter().find(|n| n.id == start.node_id)?;
        // Boundary ports route the interface, not literal values.
        if node.boundary {
            return None;
        }
        // Don't overwrite an existing const value.
        if matches!(
            scene.bindings(node.inputs).get(start.port_idx),
            Some(InputBindingView::Const(_))
        ) {
            return None;
        }
        let default = scene.defaults(node.inputs).get(start.port_idx)?.clone()?;
        Some(set_input(start, Binding::Const(default)))
    }

    /// Compatible-kind port currently snapped under the pointer
    /// during an active drag, or `None`. Read by `GraphUI` to force
    /// the hover state in `PortFrame` (otherwise palantir's
    /// drag-capture suppression would hide it).
    pub(crate) fn snap_port(&self) -> Option<PortRef> {
        self.drag.and_then(|d| d.snap_end)
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
                probe.state.as_deref_mut().unwrap().broken.push(InputPort {
                    node_id: c.tgt_node,
                    port_idx: c.tgt_port,
                });
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
        let Some(drag) = self.drag else { return };
        let Some(start) = port_frame.center_canvas_local(drag.start) else {
            return;
        };
        let end = match drag.snap_end {
            Some(snap) => port_frame.center_canvas_local(snap),
            None => ui.pointer_pos().map(|p| to_world(p - canvas_origin, scene)),
        };
        let Some(end) = end else { return };
        // Orient handles by kind: outputs grow rightward, inputs grow
        // leftward. Same dx algebra as `draw` so the preview matches
        // the eventual permanent curve exactly when snapped.
        let (p0, p3) = match drag.start.kind {
            PortKind::Output => (start, end),
            PortKind::Input => (end, start),
        };
        let CubicHandles { p1, p2 } = cubic_handles(p0, p3);
        // Tint the in-flight wire by the dragged port's data type, so the
        // preview already reads as the type being connected.
        let drag_ty = port_data_type(scene, drag.start).unwrap_or_default();
        let wire = port_color(ctx.theme, &drag_ty, drag.start.kind, false);
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

/// First port whose response shows `drag_started` this frame, or
/// `None`. Iterates inputs first then outputs per node so the topmost
/// recorded port wins ties (matches paint order). Returns a fully-
/// initialized [`ConnectionDrag`] with no snap target yet.
fn scan_drag_start(frame: &PortFrame, scene: &Scene) -> Option<ConnectionDrag> {
    for n in &scene.nodes {
        for kind in [PortKind::Input, PortKind::Output] {
            for port in node_ports(n, kind) {
                if frame.drag_started(port) {
                    return Some(ConnectionDrag {
                        start: port,
                        snap_end: None,
                    });
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
fn scan_snap_target(frame: &PortFrame, ui: &Ui, scene: &Scene, start: PortRef) -> Option<PortRef> {
    let want_kind = start.kind.opposite();
    let pointer = ui.pointer_pos()?;
    // A passthrough (subgraph input boundary wired straight to the output
    // boundary) leaves the relayed value untyped at execution and panics
    // the worker — disallow it by never snapping one boundary node onto
    // the other. The only boundary→boundary link possible is exactly that
    // passthrough, so a blanket reject is precise.
    let start_boundary = scene
        .nodes
        .iter()
        .find(|n| n.id == start.node_id)
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
            if frame.contains_pointer(port, pointer) {
                // Reject a drop onto an incompatible port so the wire
                // won't latch. Geometrically only one port sits under the
                // pointer, so a reject here falls through to `None` (drop)
                // rather than snapping elsewhere.
                let compatible = match (&start_type, port_data_type(scene, port)) {
                    (Some(a), Some(b)) => types_compatible(a, &b),
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

/// The declared [`DataType`] of `port` in the current scene, or `None`
/// if the port isn't present (e.g. mid-rebuild).
fn port_data_type(scene: &Scene, port: PortRef) -> Option<DataType> {
    let node = scene.nodes.iter().find(|n| n.id == port.node_id)?;
    let ty = match port.kind {
        PortKind::Input => scene.input_types(node.inputs).get(port.port_idx)?,
        PortKind::Output => scene.output_types(node.outputs).get(port.port_idx)?,
    };
    Some(ty.clone())
}

/// Whether an output↔input link is allowed by type. Equal types connect;
/// `Null` is a wildcard on either side, because the default type and the
/// boundary "+" placeholder are `Null` and must accept any producer so
/// wiring one grows the interface from the connected type.
fn types_compatible(a: &DataType, b: &DataType) -> bool {
    matches!(a, DataType::Null) || matches!(b, DataType::Null) || a == b
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
        to: Binding::Bind(OutputPort {
            node_id: output.node_id,
            port_idx: output.port_idx,
        }),
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use scenarium::data::{TypeDef, TypeId};

    use super::*;

    fn custom(id: u128) -> DataType {
        DataType::Custom(Arc::new(TypeDef {
            type_id: TypeId::from_u128(id),
            display_name: "X".into(),
        }))
    }

    #[test]
    fn matching_scalar_types_connect_mismatched_do_not() {
        assert!(types_compatible(&DataType::Float, &DataType::Float));
        assert!(types_compatible(&DataType::String, &DataType::String));
        assert!(!types_compatible(&DataType::Float, &DataType::Int));
        assert!(!types_compatible(&DataType::String, &DataType::Bool));
    }

    #[test]
    fn null_is_a_wildcard_both_directions() {
        assert!(types_compatible(&DataType::Null, &DataType::Float));
        assert!(types_compatible(&DataType::Float, &DataType::Null));
        assert!(types_compatible(&DataType::Null, &DataType::Null));
    }

    #[test]
    fn custom_types_match_by_id() {
        assert!(types_compatible(&custom(1), &custom(1)));
        assert!(!types_compatible(&custom(1), &custom(2)));
        assert!(!types_compatible(&custom(1), &DataType::Float));
    }
}
