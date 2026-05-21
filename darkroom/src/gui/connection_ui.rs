use glam::Vec2;
use palantir::{LineCap, Shape, Ui};
use scenarium::graph::{Binding, PortAddress};

use crate::app::AppContext;
use crate::gui::breaker::BreakerProbe;
use crate::gui::graph_ui::{PortFrame, to_world};
use crate::gui::{PortKind, PortRef};
use crate::intent::Intent;
use crate::scene::Scene;

/// Owns the in-flight new-connection drag plus the existing-connection
/// renderer. Single-port-at-a-time means one `Option` is enough; the
/// permanent connection list lives on `Scene` and is iterated each
/// frame by [`Self::draw`]. Mirrors the deprecated
/// `Gesture::DraggingConnection` but as a dedicated UI module.
#[derive(Default, Debug)]
pub struct ConnectionUI {
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
    /// Release: if a compatible `snap_end` is set, push an
    /// [`Intent::SetInput`] binding the input port to the output port.
    /// Otherwise drop silently. Esc cancels without emitting anything.
    pub fn apply(
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
        }
        self.drag = None;
    }

    /// Compatible-kind port currently snapped under the pointer
    /// during an active drag, or `None`. Read by `GraphUI` to force
    /// the hover state in `PortFrame` (otherwise palantir's
    /// drag-capture suppression would hide it).
    pub fn snap_port(&self) -> Option<PortRef> {
        self.drag.and_then(|d| d.snap_end)
    }

    /// Paint every permanent connection on the current scene, marking
    /// those the active breaker (`probe.state`) crosses as broken.
    /// Hits get pushed onto `probe.state.broken` for the breaker's
    /// release-frame drain.
    pub fn draw(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        canvas_origin: Vec2,
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
                port_frame.center_canvas_local(src_port, canvas_origin),
                port_frame.center_canvas_local(tgt_port, canvas_origin),
            ) else {
                continue;
            };
            let dx = ((p3.x - p0.x).abs() * 0.5).max(40.0);
            let p1 = p0 + Vec2::new(dx, 0.0);
            let p2 = p3 - Vec2::new(dx, 0.0);
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
                    .push(PortAddress {
                        target_id: c.tgt_node,
                        port_idx: c.tgt_port,
                    });
            }
            let color = if broken {
                ctx.theme.connection_broken
            } else {
                ctx.theme.connection
            };
            ui.add_shape(Shape::CubicBezier {
                p0,
                p1,
                p2,
                p3,
                width,
                brush: color.into(),
                cap: LineCap::Round,
            });
        }
    }

    /// Paint the in-flight drag preview: cubic from the start port's
    /// center to either the snapped target's center (when set) or the
    /// pointer position. Drawn inside the inner canvas so coordinates
    /// share the pan/zoom transform with permanent connections.
    pub fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        canvas_origin: Vec2,
    ) {
        let Some(drag) = self.drag else { return };
        let Some(start) = port_frame.center_canvas_local(drag.start, canvas_origin) else {
            return;
        };
        let end = match drag.snap_end {
            Some(snap) => port_frame.center_canvas_local(snap, canvas_origin),
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
        let dx = ((p3.x - p0.x).abs() * 0.5).max(40.0);
        ui.add_shape(Shape::CubicBezier {
            p0,
            p1: p0 + Vec2::new(dx, 0.0),
            p2: p3 - Vec2::new(dx, 0.0),
            p3,
            width: ctx.theme.connection_width,
            brush: ctx.theme.connection.into(),
            cap: LineCap::Round,
        });
    }
}

/// First port whose response shows `drag_started` this frame, or
/// `None`. Iterates inputs first then outputs per node so the topmost
/// recorded port wins ties (matches paint order). Returns a fully-
/// initialized [`ConnectionDrag`] with no snap target yet.
fn scan_drag_start(frame: &PortFrame, scene: &Scene) -> Option<ConnectionDrag> {
    for n in &scene.nodes {
        for kind in [PortKind::Input, PortKind::Output] {
            let count = match kind {
                PortKind::Input => scene.ports(n.inputs).len(),
                PortKind::Output => scene.ports(n.outputs).len(),
            };
            for port_idx in 0..count {
                let port = PortRef {
                    node_id: n.id,
                    kind,
                    port_idx,
                };
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
    for n in &scene.nodes {
        if n.id == start.node_id {
            continue;
        }
        let count = match want_kind {
            PortKind::Input => scene.ports(n.inputs).len(),
            PortKind::Output => scene.ports(n.outputs).len(),
        };
        for port_idx in 0..count {
            let port = PortRef {
                node_id: n.id,
                kind: want_kind,
                port_idx,
            };
            if frame.contains_pointer(port, pointer) {
                return Some(port);
            }
        }
    }
    None
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
        to: Binding::Bind(PortAddress {
            target_id: output.node_id,
            port_idx: output.port_idx,
        }),
    });
}
