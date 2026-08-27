use glam::Vec2;
use palantir::Ui;
use scenarium::{NodeId, Subscription};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::EventRef;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::graph::node::port_color::event_color;
use crate::gui::pane::graph::paint::wire::{GlyphDrag, Wire, WirePass, WireTint};
use crate::gui::requests::Requests;

/// Owns the in-flight subscription wire — an emitter *or* subscriber drag.
/// One wire at a time, so a single `Option` suffices. The committed wires
/// live on the authoring graph and are painted straight off it by the
/// module-level [`draw`], which needs nothing from here.
///
/// A sibling of [`crate::gui::pane::graph::gesture::connection::ConnectionUI`] rather
/// than a mode of it: an event wire carries no data type, runs no cycle /
/// const checks, and links an emitter event glyph to a whole-node
/// subscription pin (which only sink nodes expose — that's what makes
/// "events connect only to subscribers" structural). What the two *do* share
/// — latching, the release edge, pane ownership, the preview's free end — is
/// [`GlyphDrag`]'s. The drag can start from either end (mirroring a data
/// wire's start-from-input-or-output): pull from an emitter and drop on a pin,
/// or pull from a pin and drop on an emitter. Held-drag only; no const-drop or
/// new-node spawn.
#[derive(Default, Debug)]
pub(crate) struct SubscriptionUI {
    state: GestureSlot<InFlight>,
}

/// The in-flight event wire, discriminated by which end it started from —
/// which is also what flips the drag's two glyph domains, so the two
/// directions are distinct [`GlyphDrag`] types rather than one with a flag.
/// Both commit the same `SetSubscription { subscribe: true }`.
#[derive(Clone, Copy, Debug)]
enum InFlight {
    /// Started on an emitter event glyph; snapping to a subscription pin.
    FromEmitter(GlyphDrag<EventRef, NodeId>),
    /// Started on a subscription pin; snapping to an emitter event glyph.
    FromSubscriber(GlyphDrag<NodeId, EventRef>),
}

impl InFlight {
    /// The node at the drag's *fixed* end — whichever glyph the press
    /// latched. Its graph is the pane that owns the gesture.
    fn node(self) -> NodeId {
        match self {
            InFlight::FromEmitter(drag) => drag.node(),
            InFlight::FromSubscriber(drag) => drag.node(),
        }
    }
}

impl SubscriptionUI {
    /// Drop the in-flight event wire.
    pub(crate) fn reset(&mut self) {
        self.state.clear();
    }

    /// Whether a subscription-wire gesture is in flight — feeds the
    /// wire-fade tier. (A method, not a `pub(crate)` field: `InFlight` is
    /// module-private.)
    pub(crate) fn is_dragging(&self) -> bool {
        self.state.get().is_some()
    }

    /// Drive the in-flight subscription wire: latch a fresh drag from either
    /// an emitter glyph or a subscription pin, track the snapped opposite
    /// end, and commit a `SetSubscription { subscribe: true }` on release over
    /// a valid target. The context's Esc — resolved once by the canvas —
    /// drops the wire.
    ///
    /// Swept over the whole scene once per frame — one press, one wire —
    /// but the snap scans and the commit run against the pane holding the
    /// drag's fixed end, so a subscription can't span two graphs.
    pub(crate) fn apply(&mut self, ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Requests) {
        let (graph_ctx, geometry) = (cx.graph_ctx(), cx.geometry());
        // Latch a fresh drag only when idle. An emitter and a pin can't both
        // start one this frame (distinct widget-id spaces, one press), so
        // trying the emitter scan first is arbitrary, not a conflict.
        if self.state.is_idle() {
            let emitters = graph_ctx.nodes().flat_map(NodeCtx::event_refs);
            // Only sink nodes render a pin, so only they can start a reverse
            // event drag.
            let pins = graph_ctx.nodes().filter(|n| n.sink()).map(|n| n.id);
            let latched = GlyphDrag::latch(&geometry.events, emitters)
                .map(InFlight::FromEmitter)
                .or_else(|| GlyphDrag::latch(&geometry.subs, pins).map(InFlight::FromSubscriber));
            if let Some(latched) = latched
                && graph_ctx.contains(latched.node())
            {
                self.state.latch(latched);
            }
        }
        if cx.cancelled() {
            self.state.clear();
        }
        let Some(mut state) = self.state.take() else {
            return;
        };
        // A pane closed mid-drag, or a fixed end deleted under it, drops
        // the wire — not re-latching is how.
        if !graph_ctx.contains(state.node()) {
            return;
        }
        // Refresh the snapped opposite end, then read the source glyph's drag
        // state off its own layer: its transition out of `dragging` is the
        // release edge.
        let released = match &mut state {
            InFlight::FromEmitter(drag) => {
                drag.snap = scan_sub_target(geometry, ui, graph_ctx, drag.from);
                !drag.held(&geometry.events)
            }
            InFlight::FromSubscriber(drag) => {
                drag.snap = scan_emitter_target(geometry, ui, graph_ctx, drag.from);
                !drag.held(&geometry.subs)
            }
        };
        self.state.latch(state);
        if !released {
            return;
        }
        // Released over a valid target: both directions resolve to the same
        // (emitter, subscriber) pair and commit the same idempotent intent.
        match state {
            InFlight::FromEmitter(GlyphDrag {
                from: emitter,
                snap: Some(subscriber),
            })
            | InFlight::FromSubscriber(GlyphDrag {
                from: subscriber,
                snap: Some(emitter),
            }) => out.push_graph(GraphIntent::SetSubscription {
                subscription: Subscription {
                    emitter: emitter.node_id,
                    event_idx: emitter.event_idx,
                    subscriber,
                },
                subscribe: true,
            }),
            _ => {}
        }
        self.state.clear();
    }

    /// Force the hover flag on the glyph the wire is currently snapped to —
    /// the subscription pin for an emitter-started drag, the emitter glyph for
    /// a subscriber-started one — so it paints as the drop target. Palantir
    /// suppresses `response.hovered` on every widget but the drag-capture
    /// owner while a drag is live, so without this the snapped-but-not-
    /// captured target stays at its idle color.
    pub(crate) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        match self.state.get() {
            Some(InFlight::FromEmitter(drag)) => {
                if let Some(sub) = drag.snap {
                    geometry.subs.set_hovered(sub);
                }
            }
            Some(InFlight::FromSubscriber(drag)) => {
                if let Some(emitter) = drag.snap {
                    geometry.events.set_hovered(emitter);
                }
            }
            None => {}
        }
    }

    /// Paint the in-flight drag preview: a cubic between the emitter side
    /// (`p0`) and the subscriber side (`p3`). Whichever end the drag started
    /// from is fixed to its glyph; the free end follows the snapped opposite
    /// glyph (when set) or the pointer. The emitter is always `p0` so the
    /// preview keeps a committed wire's shape regardless of drag direction.
    pub(crate) fn draw_in_flight(&self, ui: &mut Ui, cx: CanvasCtx<'_>, canvas_origin: Vec2) {
        let (graph_ctx, geometry) = (cx.graph_ctx(), cx.geometry());
        // Scoped to the pane holding the drag's fixed end — see
        // `ConnectionUI::draw_in_flight` for what an unscoped preview
        // paints on the neighbouring canvases.
        let (p0, p3) = match self.state.get().copied() {
            None => return,
            Some(InFlight::FromEmitter(drag)) => {
                let Some(p0) = geometry.events.center(drag.from) else {
                    return;
                };
                let Some(p3) = drag.free_end(ui, graph_ctx, canvas_origin, &geometry.subs) else {
                    return;
                };
                (p0, p3)
            }
            Some(InFlight::FromSubscriber(drag)) => {
                let Some(p3) = geometry.subs.center(drag.from) else {
                    return;
                };
                let Some(p0) = drag.free_end(ui, graph_ctx, canvas_origin, &geometry.events) else {
                    return;
                };
                (p0, p3)
            }
        };
        let theme = graph_ctx.theme();
        Wire::event(p0, p3).add(ui, theme.stroke_width, event_color(theme, false));
    }
}

/// Paint every committed subscription wire on the current scene retained by
/// the pass's cull region, marking those the active breaker crosses as broken
/// via `probe.mark_broken_subscription` for the breaker's release-frame drain.
///
/// Reads nothing but committed scene state off `pass`, so — like
/// [`crate::gui::pane::graph::gesture::connection::draw`] — it belongs to the module
/// rather than [`SubscriptionUI`].
pub(crate) fn draw(ui: &mut Ui, pass: &mut WirePass<'_, '_>) {
    let (theme, graph_ctx, geometry) =
        (pass.dcx.theme(), pass.dcx.graph_ctx(), pass.dcx.geometry());
    for s in graph_ctx.subscriptions() {
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
        let hover = geometry.events.is_hovered(emitter) || geometry.subs.is_hovered(s.subscriber);
        let wire = Wire::event(p0, p3);
        // Events carry no data type, so one neutral swatch runs the whole
        // curve rather than a per-end gradient.
        if pass.draw_wire(ui, &wire, hover, || {
            WireTint::flat(event_color(theme, false))
        }) {
            pass.probe.mark_broken_subscription(s);
        }
    }
}

/// Subscription pin under the pointer that's a valid drop for `emitter`: a
/// sink node (the only kind that renders a pin) other than the emitter's
/// own node. The pin-only target enforces "events connect only to
/// subscribers"; the self-node skip rejects a node subscribing to itself.
fn scan_sub_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    emitter: EventRef,
) -> Option<NodeId> {
    let pointer = ui.pointer_pos()?;
    let candidates = graph_ctx
        .nodes()
        .filter(|n| n.id != emitter.node_id && n.sink())
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
    graph_ctx: GraphCtx<'_>,
    subscriber: NodeId,
) -> Option<EventRef> {
    let pointer = ui.pointer_pos()?;
    let candidates = graph_ctx
        .nodes()
        .filter(|n| n.id != subscriber)
        .flat_map(NodeCtx::event_refs);
    geometry.events.first_containing(pointer, candidates)
}
