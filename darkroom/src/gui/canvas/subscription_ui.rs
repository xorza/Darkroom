use aperture::{Brush, Rect, Ui};
use glam::Vec2;
use scenarium::graph::NodeId;

use crate::core::edit::intent::types::Intent;
use crate::gui::EventRef;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::wire_visible;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::pointer_world;
use crate::gui::canvas::wire::{CubicHandles, MIN_HANDLE, WireEmphasis, add_cubic_wire};
use crate::gui::node::port_color::event_color;
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

/// Owns the in-flight subscription wire (an emitter *or* subscriber drag)
/// plus the committed subscription-wire renderer. One wire at a time, so a
/// single `Option` suffices; the committed list lives on
/// `Scene::subscriptions`.
///
/// A sibling of [`crate::gui::canvas::connection_ui::ConnectionUI`] rather
/// than a mode of it: an event wire carries no data type, runs no cycle /
/// const checks, and links an emitter event glyph to a whole-node
/// subscription pin (which only sink nodes expose — that's what makes
/// "events connect only to subscribers" structural). The drag can start from
/// either end (mirroring a data wire's start-from-input-or-output): pull from
/// an emitter and drop on a pin, or pull from a pin and drop on an emitter.
/// Held-drag only; no const-drop or new-node spawn.
#[derive(Default, Debug)]
pub(crate) struct SubscriptionUI {
    state: Option<InFlight>,
}

/// The in-flight event wire, discriminated by which end it started from. Both
/// directions commit the same `SetSubscription { subscribe: true }`; only the
/// fixed end and the snap target differ. Identity-only — endpoints resolve every
/// frame from `CanvasGeometry`, so the wire survives layout changes and node moves.
#[derive(Clone, Copy, Debug)]
enum InFlight {
    /// Started on an emitter event glyph; snapping to a subscription pin.
    FromEmitter {
        emitter: EventRef,
        /// Subscription pin currently under the pointer, if any.
        snap_sub: Option<NodeId>,
    },
    /// Started on a subscription pin; snapping to an emitter event glyph.
    FromSubscriber {
        subscriber: NodeId,
        /// Emitter event glyph currently under the pointer, if any.
        snap_emitter: Option<EventRef>,
    },
}

impl SubscriptionUI {
    /// Whether a subscription-wire gesture is in flight — feeds the shared
    /// wire-fade tier. (A method, not a `pub(crate)` field: `InFlight` is
    /// module-private.)
    pub(crate) fn dragging(&self) -> bool {
        self.state.is_some()
    }

    /// Drive the in-flight subscription wire: latch a fresh drag from either
    /// an emitter glyph or a subscription pin, track the snapped opposite
    /// end, and commit a `SetSubscription { subscribe: true }` on release over
    /// a valid target. Esc cancels.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        out: &mut Vec<Intent>,
    ) {
        // Latch a fresh drag only when idle. An emitter and a pin can't both
        // start one this frame (distinct widget-id spaces, one press), so
        // preferring the emitter scan is arbitrary, not a conflict.
        if self.state.is_none() {
            if let Some(emitter) = scan_event_drag_start(geometry, scene) {
                self.state = Some(InFlight::FromEmitter {
                    emitter,
                    snap_sub: None,
                });
            } else if let Some(subscriber) = scan_sub_drag_start(geometry, scene) {
                self.state = Some(InFlight::FromSubscriber {
                    subscriber,
                    snap_emitter: None,
                });
            }
        }
        if ui.escape_pressed() {
            self.state = None;
            return;
        }
        let Some(mut state) = self.state else {
            return;
        };
        // Refresh the snapped opposite end, then read the source glyph's drag
        // state: `*_dragging` rolls up `drag_delta().is_some() ||
        // drag_started()`, so its transition to `false` is the release edge.
        let still_dragging = match &mut state {
            InFlight::FromEmitter { emitter, snap_sub } => {
                *snap_sub = scan_sub_target(geometry, ui, scene, *emitter);
                geometry.events.dragging(*emitter)
            }
            InFlight::FromSubscriber {
                subscriber,
                snap_emitter,
            } => {
                *snap_emitter = scan_emitter_target(geometry, ui, scene, *subscriber);
                geometry.subs.dragging(*subscriber)
            }
        };
        self.state = Some(state);
        if still_dragging {
            return;
        }
        // Released over a valid target: both directions resolve to the same
        // (emitter, subscriber) pair and commit the same idempotent intent.
        match state {
            InFlight::FromEmitter {
                emitter,
                snap_sub: Some(subscriber),
            }
            | InFlight::FromSubscriber {
                subscriber,
                snap_emitter: Some(emitter),
            } => out.push(Intent::SetSubscription {
                emitter: emitter.node_id,
                event_idx: emitter.event_idx,
                subscriber,
                subscribe: true,
            }),
            _ => {}
        }
        self.state = None;
    }

    /// The subscription pin currently snapped under the pointer (an
    /// emitter-started drag), if any — read by `GraphUI` to highlight the
    /// drop target.
    pub(crate) fn snap_sub(&self) -> Option<NodeId> {
        match self.state {
            Some(InFlight::FromEmitter { snap_sub, .. }) => snap_sub,
            _ => None,
        }
    }

    /// The emitter event glyph currently snapped under the pointer (a
    /// subscriber-started drag), if any — read by `GraphUI` to highlight the
    /// drop target.
    pub(crate) fn snap_emitter(&self) -> Option<EventRef> {
        match self.state {
            Some(InFlight::FromSubscriber { snap_emitter, .. }) => snap_emitter,
            _ => None,
        }
    }

    /// Paint every committed subscription wire on the current scene that
    /// can intersect `visible`, marking those the active breaker crosses as
    /// broken via `probe.mark_broken_subscription` for the breaker's
    /// release-frame drain. A culled wire skips the breaker probe too — the
    /// scribble is always on-screen, so it can't cross an off-screen curve.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        visible: Option<Rect>,
        probe: &mut BreakerProbe<'_>,
        emphasis: &WireEmphasis,
    ) {
        let width = ctx.theme.connection_width;
        for s in &scene.subscriptions {
            let emitter = EventRef {
                node_id: s.emitter,
                event_idx: s.event_idx,
            };
            let (Some(p0), Some(p3)) = (
                geometry.events.center(emitter),
                geometry.subs.center(s.subscriber),
            ) else {
                continue;
            };
            let handles = event_handles(p0, p3);
            if !wire_visible(visible, p0, &handles, p3) {
                continue;
            }
            let broken = probe.crosses_cubic(p0, handles.p1, handles.p2, p3);
            if broken {
                probe.mark_broken_subscription(*s);
            }
            // Emphasis tiers resolve through the shared `WireEmphasis` (see
            // wire.rs). Event wires share the breaker-alarm hue, so the
            // alarm read on a broken wire is full strength + full width
            // against the breaker-faded rest of the set.
            let endpoint_hover =
                geometry.events.is_hovered(emitter) || geometry.subs.is_hovered(s.subscriber);
            let hovered = !broken && emphasis.hovered(endpoint_hover);
            let brush = if broken {
                Brush::Solid(ctx.theme.colors.connection_broken)
            } else {
                Brush::Solid(emphasis.tint(event_color(ctx.theme, false), hovered))
            };
            let w = emphasis.width(width, hovered || broken);
            add_cubic_wire(ui, p0, p3, handles, w, brush);
        }
    }

    /// Paint the in-flight drag preview: a cubic between the emitter side
    /// (`p0`) and the subscriber side (`p3`). Whichever end the drag started
    /// from is fixed to its glyph; the free end follows the snapped opposite
    /// glyph (when set) or the pointer. The emitter is always `p0` so the
    /// preview keeps a committed wire's shape regardless of drag direction.
    pub(crate) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        canvas_origin: Vec2,
    ) {
        let (p0, p3) = match self.state {
            None => return,
            Some(InFlight::FromEmitter { emitter, snap_sub }) => {
                let Some(p0) = geometry.events.center(emitter) else {
                    return;
                };
                let free = match snap_sub {
                    Some(sub) => geometry.subs.center(sub),
                    None => pointer_world(ui, scene, canvas_origin),
                };
                let Some(p3) = free else { return };
                (p0, p3)
            }
            Some(InFlight::FromSubscriber {
                subscriber,
                snap_emitter,
            }) => {
                let Some(p3) = geometry.subs.center(subscriber) else {
                    return;
                };
                let free = match snap_emitter {
                    Some(e) => geometry.events.center(e),
                    None => pointer_world(ui, scene, canvas_origin),
                };
                let Some(p0) = free else { return };
                (p0, p3)
            }
        };
        add_cubic_wire(
            ui,
            p0,
            p3,
            event_handles(p0, p3),
            ctx.theme.connection_width,
            Brush::Solid(event_color(ctx.theme, false)),
        );
    }
}

/// First emitter event glyph whose drag started this frame, or `None`.
fn scan_event_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<EventRef> {
    let keys = scene.nodes.iter().flat_map(|n| {
        (0..n.events.len as usize).map(move |event_idx| EventRef {
            node_id: n.id,
            event_idx,
        })
    });
    geometry.events.first_drag_started(keys)
}

/// First subscription pin whose drag started this frame, or `None`. Only
/// sink nodes render a pin, so only they can start a reverse event drag.
fn scan_sub_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<NodeId> {
    let keys = scene.nodes.iter().filter(|n| n.sink).map(|n| n.id);
    geometry.subs.first_drag_started(keys)
}

/// Subscription pin under the pointer that's a valid drop for `emitter`: a
/// sink node (the only kind that renders a pin) other than the emitter's
/// own node. The pin-only target enforces "events connect only to
/// subscribers"; the self-node skip rejects a node subscribing to itself.
fn scan_sub_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    scene: &Scene,
    emitter: EventRef,
) -> Option<NodeId> {
    let pointer = ui.pointer_pos()?;
    for n in &scene.nodes {
        if n.id == emitter.node_id || !n.sink {
            continue;
        }
        if geometry.subs.contains_pointer(n.id, pointer) {
            return Some(n.id);
        }
    }
    None
}

/// Emitter event glyph under the pointer that's a valid drop for a wire
/// dragged from `subscriber`'s pin: any node's event other than the
/// subscriber's own (a node can't subscribe to itself). Mirror of
/// [`scan_sub_target`] for the reverse drag.
fn scan_emitter_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    scene: &Scene,
    subscriber: NodeId,
) -> Option<EventRef> {
    let pointer = ui.pointer_pos()?;
    for n in &scene.nodes {
        if n.id == subscriber {
            continue;
        }
        for event_idx in 0..n.events.len as usize {
            let e = EventRef {
                node_id: n.id,
                event_idx,
            };
            if geometry.events.contains_pointer(e, pointer) {
                return Some(e);
            }
        }
    }
    None
}
