use glam::Vec2;
use palantir::{CurveBrush, Ui};
use scenarium::NodeId;

use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::EventRef;
use crate::gui::app::AppContext;
use crate::gui::canvas::free_end;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::wire::{Wire, WirePass};
use crate::gui::node::port_color::event_color;
use crate::gui::scene::{GraphScene, Scene, SceneNode};

/// Owns the in-flight subscription wire — an emitter *or* subscriber drag.
/// One wire at a time, so a single `Option` suffices. The committed wires
/// live on `Scene::subscriptions` and are painted by the module-level
/// [`draw`], which needs nothing from here.
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
pub(super) struct SubscriptionUI {
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

impl InFlight {
    /// The node at the drag's *fixed* end — whichever glyph the press
    /// latched on. Its graph is the pane that owns the gesture.
    fn anchor_node(self) -> NodeId {
        match self {
            InFlight::FromEmitter { emitter, .. } => emitter.node_id,
            InFlight::FromSubscriber { subscriber, .. } => subscriber,
        }
    }
}

impl SubscriptionUI {
    /// Whether a subscription-wire gesture is in flight — feeds the shared
    /// wire-fade tier. (A method, not a `pub(super)` field: `InFlight` is
    /// module-private.)
    pub(super) fn dragging(&self) -> bool {
        self.state.is_some()
    }

    /// Drive the in-flight subscription wire: latch a fresh drag from either
    /// an emitter glyph or a subscription pin, track the snapped opposite
    /// end, and commit a `SetSubscription { subscribe: true }` on release over
    /// a valid target. Esc cancels.
    ///
    /// Swept over the whole scene once per frame — one press, one wire —
    /// but the snap scans and the commit run against the pane holding the
    /// drag's fixed end, so a subscription can't span two graphs.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        out: &mut Intents,
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
        // The pane holding the drag's fixed end owns the gesture — its
        // nodes are the only snap candidates, and its target is what the
        // commit lands on. A pane closed mid-drag drops the wire.
        let Some(graph) = scene.owner(state.anchor_node()) else {
            self.state = None;
            return;
        };
        // Refresh the snapped opposite end, then read the source glyph's drag
        // state: `*_dragging` rolls up `drag_delta().is_some() ||
        // drag_started()`, so its transition to `false` is the release edge.
        let still_dragging = match &mut state {
            InFlight::FromEmitter { emitter, snap_sub } => {
                *snap_sub = scan_sub_target(geometry, ui, graph, *emitter);
                geometry.events.dragging(*emitter)
            }
            InFlight::FromSubscriber {
                subscriber,
                snap_emitter,
            } => {
                *snap_emitter = scan_emitter_target(geometry, ui, graph, *subscriber);
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
            } => out.for_graph(graph.target(), |out| {
                out.push(Intent::SetSubscription {
                    emitter: emitter.node_id,
                    event_idx: emitter.event_idx,
                    subscriber,
                    subscribe: true,
                })
            }),
            _ => {}
        }
        self.state = None;
    }

    /// The subscription pin currently snapped under the pointer (an
    /// emitter-started drag), if any — read by `GraphUI` to highlight the
    /// drop target.
    pub(super) fn snap_sub(&self) -> Option<NodeId> {
        match self.state {
            Some(InFlight::FromEmitter { snap_sub, .. }) => snap_sub,
            _ => None,
        }
    }

    /// The emitter event glyph currently snapped under the pointer (a
    /// subscriber-started drag), if any — read by `GraphUI` to highlight the
    /// drop target.
    pub(super) fn snap_emitter(&self) -> Option<EventRef> {
        match self.state {
            Some(InFlight::FromSubscriber { snap_emitter, .. }) => snap_emitter,
            _ => None,
        }
    }

    /// Paint the in-flight drag preview: a cubic between the emitter side
    /// (`p0`) and the subscriber side (`p3`). Whichever end the drag started
    /// from is fixed to its glyph; the free end follows the snapped opposite
    /// glyph (when set) or the pointer. The emitter is always `p0` so the
    /// preview keeps a committed wire's shape regardless of drag direction.
    pub(super) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        canvas_origin: Vec2,
    ) {
        let (p0, p3) = match self.state {
            None => return,
            Some(InFlight::FromEmitter { emitter, snap_sub }) => {
                let Some(p0) = geometry.events.center(emitter) else {
                    return;
                };
                let Some(p3) = free_end(ui, graph, canvas_origin, &geometry.subs, snap_sub) else {
                    return;
                };
                (p0, p3)
            }
            Some(InFlight::FromSubscriber {
                subscriber,
                snap_emitter,
            }) => {
                let Some(p3) = geometry.subs.center(subscriber) else {
                    return;
                };
                let Some(p0) = free_end(ui, graph, canvas_origin, &geometry.events, snap_emitter)
                else {
                    return;
                };
                (p0, p3)
            }
        };
        Wire::event(p0, p3).add(
            ui,
            ctx.theme.connection_width,
            CurveBrush::Solid(event_color(ctx.theme, false)),
        );
    }
}

/// Paint every committed subscription wire on the current scene retained by
/// the pass's cull region, marking those the active breaker crosses as broken
/// via `probe.mark_broken_subscription` for the breaker's release-frame drain.
///
/// Reads nothing but committed scene state off `pass`, so — like
/// [`crate::gui::canvas::connection_ui::draw`] — it belongs to the module
/// rather than [`SubscriptionUI`].
pub(super) fn draw(ui: &mut Ui, pass: &mut WirePass<'_, '_>) {
    let (theme, graph) = (pass.theme, pass.graph);
    for s in graph.subscriptions() {
        let emitter = EventRef {
            node_id: s.emitter,
            event_idx: s.event_idx,
        };
        let (Some(p0), Some(p3)) = (
            pass.geometry.events.center(emitter),
            pass.geometry.subs.center(s.subscriber),
        ) else {
            continue;
        };
        let wire = Wire::event(p0, p3);
        let endpoint_hover =
            pass.geometry.events.is_hovered(emitter) || pass.geometry.subs.is_hovered(s.subscriber);
        let Some(stroke) = pass.resolve(&wire, endpoint_hover) else {
            continue;
        };
        if stroke.broken {
            pass.probe.mark_broken_subscription(*s);
        }
        // Event wires share the breaker-alarm hue, so a broken one paints
        // flat rather than tinted — full strength against the
        // breaker-faded rest of the set.
        let brush = if stroke.broken {
            CurveBrush::Solid(theme.colors.connection_broken)
        } else {
            CurveBrush::Solid(
                pass.emphasis
                    .tint(event_color(theme, false), stroke.hovered),
            )
        };
        wire.add(ui, stroke.width, brush);
    }
}

/// First emitter event glyph whose drag started this frame, or `None`.
fn scan_event_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<EventRef> {
    let keys = scene.nodes.values().flat_map(SceneNode::events);
    geometry.events.first_drag_started(keys)
}

/// First subscription pin whose drag started this frame, or `None`. Only
/// sink nodes render a pin, so only they can start a reverse event drag.
fn scan_sub_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<NodeId> {
    let keys = scene.nodes.values().filter(|n| n.sink).map(|n| n.id);
    geometry.subs.first_drag_started(keys)
}

/// Subscription pin under the pointer that's a valid drop for `emitter`: a
/// sink node (the only kind that renders a pin) other than the emitter's
/// own node. The pin-only target enforces "events connect only to
/// subscribers"; the self-node skip rejects a node subscribing to itself.
fn scan_sub_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    graph: GraphScene<'_>,
    emitter: EventRef,
) -> Option<NodeId> {
    let pointer = ui.pointer_pos()?;
    let candidates = graph
        .nodes()
        .filter(|n| n.id != emitter.node_id && n.sink)
        .map(|n| n.id);
    geometry.subs.first_containing(pointer, candidates)
}

/// Emitter event glyph under the pointer that's a valid drop for a wire
/// dragged from `subscriber`'s pin: any node's event other than the
/// subscriber's own (a node can't subscribe to itself). Mirror of
/// [`scan_sub_target`] for the reverse drag.
fn scan_emitter_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    graph: GraphScene<'_>,
    subscriber: NodeId,
) -> Option<EventRef> {
    let pointer = ui.pointer_pos()?;
    let candidates = graph
        .nodes()
        .filter(|n| n.id != subscriber)
        .flat_map(SceneNode::events);
    geometry.events.first_containing(pointer, candidates)
}
