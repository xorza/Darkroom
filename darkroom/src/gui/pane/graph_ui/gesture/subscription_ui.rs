use palantir::Ui;
use scenarium::{NodeId, Subscription};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::wire::WirePaint;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::frame::wire::Wire;
use crate::gui::pane::graph_ui::gesture::glyph_drag::GlyphDrag;
use scenarium::EventPort;

/// Owns the in-flight subscription wire — an emitter *or* subscriber drag.
/// One wire at a time, so a single `Option` suffices. The committed wires
/// live on the authoring graph and are painted straight off it by
/// [`WirePass::draw_event_wires`](crate::gui::pane::graph_ui::draw::wire::WirePass::draw_event_wires),
/// which needs nothing from here.
///
/// A sibling of [`crate::gui::pane::graph_ui::gesture::wire_ui::WireUi`] rather
/// than a mode of it: an event wire carries no data type, runs no cycle /
/// const checks, and links an emitter event glyph to a whole-node
/// subscription pin (which only sink nodes expose — that's what makes
/// "events connect only to subscribers" structural). What the two *do* share
/// — latching, the release edge, the fixed end's node, the preview's free
/// end — is [`GlyphDrag`]'s. The drag can start from either end (mirroring a data
/// wire's start-from-input-or-output): pull from an emitter and drop on a pin,
/// or pull from a pin and drop on an emitter. Held-drag only; no const-drop or
/// new-node spawn.
#[derive(Default, Debug)]
pub(crate) struct SubscriptionUi {
    state: Option<InFlight>,
}

/// The in-flight event wire, discriminated by which end it started from —
/// which is also what flips the drag's two glyph domains, so the two
/// directions are distinct [`GlyphDrag`] types rather than one with a flag.
/// Both commit the same `SetSubscription { to: true }`.
#[derive(Clone, Copy, Debug)]
enum InFlight {
    /// Started on an emitter event glyph; snapping to a subscription pin.
    FromEmitter(GlyphDrag<EventPort, NodeId>),
    /// Started on a subscription pin; snapping to an emitter event glyph.
    FromSubscriber(GlyphDrag<NodeId, EventPort>),
}

impl InFlight {
    /// The node at the drag's *fixed* end — whichever glyph the press
    /// latched.
    fn node(self) -> NodeId {
        match self {
            Self::FromEmitter(drag) => drag.node(),
            Self::FromSubscriber(drag) => drag.node(),
        }
    }
}

impl SubscriptionUi {
    /// Drop the in-flight event wire.
    pub(crate) const fn reset(&mut self) {
        self.state = None;
    }

    /// Whether a subscription wire is being dragged — the whole of this
    /// controller's state, so it answers both the Esc policy and the
    /// wire-fade tier. (A method, not a `pub(crate)` field: `InFlight` is
    /// module-private.)
    pub(crate) const fn in_flight(&self) -> bool {
        self.state.as_ref().is_some()
    }

    /// Drive the in-flight subscription wire: latch a fresh drag from either
    /// an emitter glyph or a subscription pin, track the snapped opposite
    /// end, and commit a `SetSubscription { to: true }` on release over
    /// a valid target. The context's Esc — resolved once by the canvas —
    /// drops the wire.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, ctx: CanvasCtx<'_>, out: &mut Requests) {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        // Latch a fresh drag only when idle. An emitter and a pin can't both
        // start one this frame (distinct widget-id spaces, one press), so
        // trying the emitter scan first is arbitrary, not a conflict.
        if self.state.is_none() {
            // Every event glyph is an emitter, and only sink nodes render a
            // pin, so whatever glyph a drag started on can start this one.
            let latched = GlyphDrag::latch(&geometry.events, |_| true)
                .map(InFlight::FromEmitter)
                .or_else(|| {
                    GlyphDrag::latch(&geometry.subs, |_| true).map(InFlight::FromSubscriber)
                });
            if let Some(latched) = latched
                && graph_ctx.contains(latched.node())
            {
                self.state = Some(latched);
            }
        }
        if ctx.cancelled() {
            self.state = None;
        }
        let Some(mut state) = self.state.take() else {
            return;
        };
        // A fixed end deleted under the drag drops the wire — not
        // re-latching is how.
        if !graph_ctx.contains(state.node()) {
            return;
        }
        // Refresh the snapped opposite end, then read the source glyph's drag
        // state off its own layer: its transition out of `dragging` is the
        // release edge.
        let released = match &mut state {
            InFlight::FromEmitter(drag) => {
                drag.snap = scan_subscription_target(ui, ctx, drag.from);
                !drag.held(&geometry.events)
            }
            InFlight::FromSubscriber(drag) => {
                drag.snap = scan_emitter_target(ui, ctx, drag.from);
                !drag.held(&geometry.subs)
            }
        };
        self.state = Some(state);
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
                subscription: Subscription::new(emitter, subscriber),
                to: true,
            }),
            _ => {}
        }
        self.state = None;
    }

    /// Force the hover flag on the glyph the wire is currently snapped to —
    /// the subscription pin for an emitter-started drag, the emitter glyph for
    /// a subscriber-started one — so it paints as the drop target. Palantir
    /// suppresses `response.hovered` on every widget but the drag-capture
    /// owner while a drag is live, so without this the snapped-but-not-
    /// captured target stays at its idle color.
    pub(crate) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        match self.state.as_ref() {
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
    pub(crate) fn draw_in_flight(&self, ui: &mut Ui, ctx: CanvasCtx<'_>) {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        let Some(state) = self.state.as_ref().copied() else {
            return;
        };
        let pointer = canvas::world_pointer(ui, graph_ctx.viewport());
        let (p0, p3) = match state {
            InFlight::FromEmitter(drag) => {
                let Some(p0) = geometry.events.center(drag.from) else {
                    return;
                };
                let Some(p3) = drag.free_end(pointer, &geometry.subs) else {
                    return;
                };
                (p0, p3)
            }
            InFlight::FromSubscriber(drag) => {
                let Some(p3) = geometry.subs.center(drag.from) else {
                    return;
                };
                let Some(p0) = drag.free_end(pointer, &geometry.events) else {
                    return;
                };
                (p0, p3)
            }
        };
        let theme = graph_ctx.theme();
        let width = theme.stroke.wire_at(graph_ctx.viewport().zoom);
        WirePaint::from(theme.port.event).stroke(ui, &Wire::event(p0, p3), width);
    }
}

/// Subscription pin under the pointer that's a valid drop for `emitter`: a
/// sink node (the only kind that renders a pin) other than the emitter's
/// own node. The pin-only target enforces "events connect only to
/// subscribers"; the self-node skip rejects a node subscribing to itself.
fn scan_subscription_target(ui: &mut Ui, ctx: CanvasCtx<'_>, emitter: EventPort) -> Option<NodeId> {
    let pointer = ui.pointer_pos()?;
    let candidates = ctx
        .graph_ctx()
        .nodes()
        .filter(|n| n.node_id != emitter.node_id && n.sink())
        .map(|n| n.node_id);
    ctx.geometry().subs.first_containing(pointer, candidates)
}

/// Emitter event glyph under the pointer that's a valid drop for a wire
/// dragged from `subscriber`'s pin: any node's event other than the
/// subscriber's own (a node can't subscribe to itself). Mirror of
/// [`scan_subscription_target`] for the reverse drag.
fn scan_emitter_target(ui: &mut Ui, ctx: CanvasCtx<'_>, subscriber: NodeId) -> Option<EventPort> {
    let pointer = ui.pointer_pos()?;
    let candidates = ctx
        .graph_ctx()
        .nodes()
        .filter(|n| n.node_id != subscriber)
        .flat_map(NodeCtx::event_ports);
    ctx.geometry().events.first_containing(pointer, candidates)
}
