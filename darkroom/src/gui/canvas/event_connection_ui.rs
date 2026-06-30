use glam::Vec2;
use palantir::{Brush, Color, Ui};
use scenarium::prelude::NodeId;

use crate::core::edit::intent::Intent;
use crate::gui::EventRef;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::pointer_world;
use crate::gui::canvas::port_frame::PortFrame;
use crate::gui::canvas::wire::{CubicHandles, MIN_HANDLE, add_cubic_wire};
use crate::gui::scene::Scene;

/// Control points for an event wire from emitter `p0` (a triangle on the
/// right of its node) to subscriber pin `p3` (the top-left pin). The emitter
/// handle leaves rightward like a data output; the subscriber handle points
/// **up-left**, matching the pin's outward-pointing triangle so the wire
/// meets it head-on.
fn event_handles(p0: Vec2, p3: Vec2) -> CubicHandles {
    let d = (p0.distance(p3) * 0.4).max(MIN_HANDLE);
    // (-1, -1) is up-left in screen space (y grows downward).
    let up_left = Vec2::new(-1.0, -1.0).normalize();
    CubicHandles {
        p1: p0 + Vec2::new(d, 0.0),
        p2: p3 + up_left * d,
    }
}

/// Owns the in-flight subscription wire (an emitter drag) plus the committed
/// subscription-wire renderer. One wire at a time, so a single `Option`
/// suffices; the committed list lives on `Scene::subscriptions`.
///
/// A sibling of [`crate::gui::canvas::connection_ui::ConnectionUI`] rather
/// than a mode of it: an event wire carries no data type, runs no cycle /
/// const checks, and snaps only to whole-node subscription pins (which only
/// terminal nodes expose — that's what makes "events connect only to
/// subscribers" structural). Held-drag only; no const-drop or new-node spawn.
#[derive(Default, Debug)]
pub(crate) struct EventConnectionUI {
    state: Option<EventInFlight>,
}

/// Identity-only — endpoints resolve every frame from `PortFrame`, so the
/// wire survives layout changes and node moves.
#[derive(Clone, Copy, Debug)]
struct EventInFlight {
    /// The emitter event the wire started from.
    emitter: EventRef,
    /// Subscription pin currently under the pointer, if any — drives the
    /// preview's snap end.
    snap_sub: Option<NodeId>,
}

impl EventConnectionUI {
    /// Drive the in-flight subscription wire: latch a fresh emitter drag,
    /// track the snapped subscription pin, and commit an
    /// [`Intent::Subscribe`] on release over a pin. Esc cancels.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        port_frame: &PortFrame,
        out: &mut Vec<Intent>,
    ) {
        if self.state.is_none()
            && let Some(emitter) = scan_event_drag_start(port_frame, scene)
        {
            self.state = Some(EventInFlight {
                emitter,
                snap_sub: None,
            });
        }
        if ui.escape_pressed() {
            self.state = None;
            return;
        }
        let Some(mut state) = self.state else {
            return;
        };
        state.snap_sub = scan_sub_target(port_frame, ui, scene, state.emitter);
        self.state = Some(state);

        // `event_dragging` rolls up `drag_delta().is_some() ||
        // drag_started()`; its transition to `false` is the release edge.
        if port_frame.event_dragging(state.emitter) {
            return;
        }
        if let Some(subscriber) = state.snap_sub {
            out.push(Intent::Subscribe {
                emitter: state.emitter.node_id,
                event_idx: state.emitter.event_idx,
                subscriber,
            });
        }
        self.state = None;
    }

    /// The subscription pin currently snapped under the pointer, if any —
    /// read by `GraphUI` to highlight the drop target.
    pub(crate) fn snap_sub(&self) -> Option<NodeId> {
        self.state.and_then(|s| s.snap_sub)
    }

    /// Paint every committed subscription wire on the current scene, marking
    /// those the active breaker (`probe.state`) crosses as broken — pushed
    /// onto `probe.state.broken_subscriptions` for the release-frame drain.
    pub(crate) fn draw(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        probe: &mut BreakerProbe<'_>,
    ) {
        let width = ctx.theme.connection_width;
        for s in &scene.subscriptions {
            let emitter = EventRef {
                node_id: s.emitter,
                event_idx: s.event_idx,
            };
            let (Some(p0), Some(p3)) = (
                port_frame.event_center_canvas_local(emitter),
                port_frame.sub_center_canvas_local(s.subscriber),
            ) else {
                continue;
            };
            let handles = event_handles(p0, p3);
            let broken = probe.crosses_cubic(p0, handles.p1, handles.p2, p3);
            if broken {
                // unwrap: `broken == true` implies `state` is `Some`.
                probe
                    .state
                    .as_deref_mut()
                    .unwrap()
                    .broken_subscriptions
                    .push(*s);
            }
            // White to match the emitter/subscriber glyphs; the broken alarm
            // color wins while the breaker crosses it.
            let brush = if broken {
                Brush::Solid(ctx.theme.connection_broken)
            } else {
                Brush::Solid(Color::WHITE)
            };
            add_cubic_wire(ui, p0, p3, handles, width, brush);
        }
    }

    /// Paint the in-flight drag preview: a cubic from the emitter glyph to
    /// the snapped pin (when set) or the pointer.
    pub(crate) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        canvas_origin: Vec2,
    ) {
        let Some(state) = self.state else {
            return;
        };
        let Some(p0) = port_frame.event_center_canvas_local(state.emitter) else {
            return;
        };
        let end = match state.snap_sub {
            Some(sub) => port_frame.sub_center_canvas_local(sub),
            None => pointer_world(ui, scene, canvas_origin),
        };
        let Some(p3) = end else {
            return;
        };
        add_cubic_wire(
            ui,
            p0,
            p3,
            event_handles(p0, p3),
            ctx.theme.connection_width,
            Brush::Solid(Color::WHITE),
        );
    }
}

/// First emitter event glyph whose drag started this frame, or `None`.
fn scan_event_drag_start(frame: &PortFrame, scene: &Scene) -> Option<EventRef> {
    for n in &scene.nodes {
        for event_idx in 0..n.events.len as usize {
            let e = EventRef {
                node_id: n.id,
                event_idx,
            };
            if frame.event_drag_started(e) {
                return Some(e);
            }
        }
    }
    None
}

/// Subscription pin under the pointer that's a valid drop for `emitter`: a
/// terminal node (the only kind that renders a pin) other than the emitter's
/// own node. The pin-only target enforces "events connect only to
/// subscribers"; the self-node skip rejects a node subscribing to itself.
fn scan_sub_target(frame: &PortFrame, ui: &Ui, scene: &Scene, emitter: EventRef) -> Option<NodeId> {
    let pointer = ui.pointer_pos()?;
    for n in &scene.nodes {
        if n.id == emitter.node_id || !n.terminal {
            continue;
        }
        if frame.sub_contains_pointer(n.id, pointer) {
            return Some(n.id);
        }
    }
    None
}
