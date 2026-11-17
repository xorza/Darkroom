use palantir::Ui;
use scenarium::{Listener, NodeId, Subscription, TriggerPort};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::wire::WirePaint;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::gesture::glyph_drag::GlyphDrag;
use crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::scene::wire::Wire;
use scenarium::EventPort;

/// Owns the in-flight subscription wire, an emitter or listener drag. One wire at a time, so a
/// single `Option` suffices. The committed wires live on the authoring graph and are painted off
/// it by
/// [`WirePass::draw_event_wires`](crate::gui::pane::graph_ui::draw::wire::WirePass::draw_event_wires).
///
/// A sibling of [`crate::gui::pane::graph_ui::gesture::wire_ui::WireUi`] rather than a mode of
/// it: an event wire carries no data type, runs no cycle or const checks, and links an emitter
/// event glyph to a listener: a pin, or a trigger an `Outputs` node hears.
/// What the two share (latching, the release edge, the fixed end's node, the preview's free end)
/// is [`GlyphDrag`]'s. The drag can start from either end: pull from an emitter and drop on a
/// pin or a trigger, or pull from either and drop on an emitter. Held-drag only.
#[derive(Default, Debug)]
pub(crate) struct SubscriptionUi {
    state: Option<InFlight>,
}

/// The in-flight event wire, discriminated by the glyph it started from, which flips the drag's
/// glyph domains. Each commits the same `SetSubscription { to: true }`.
#[derive(Clone, Copy, Debug)]
enum InFlight {
    /// Started on an emitter event glyph; snapping to a pin or a trigger.
    Emitter(GlyphDrag<EventPort, Listener>),
    /// Started on a subscription pin; snapping to an emitter event glyph.
    Pin(GlyphDrag<NodeId, EventPort>),
    /// Started on a trigger glyph; snapping to an emitter event glyph.
    Trigger(GlyphDrag<TriggerPort, EventPort>),
}

impl InFlight {
    /// The node at the drag's fixed end: whichever glyph the press latched.
    fn node(self) -> NodeId {
        match self {
            Self::Emitter(drag) => drag.node(),
            Self::Pin(drag) => drag.node(),
            Self::Trigger(drag) => drag.node(),
        }
    }

    /// The listener at the drag's fixed end, for a drag that started on one.
    const fn listener(self) -> Option<(Listener, Option<EventPort>)> {
        match self {
            Self::Emitter(_) => None,
            Self::Pin(drag) => Some((Listener::Pin(drag.from), drag.snap)),
            Self::Trigger(drag) => Some((Listener::Trigger(drag.from), drag.snap)),
        }
    }
}

impl SubscriptionUi {
    /// Drop the in-flight event wire.
    pub(crate) const fn reset(&mut self) {
        self.state = None;
    }

    /// Whether a subscription wire is being dragged, which answers both the Esc policy and the
    /// wire-fade tier.
    pub(crate) const fn in_flight(&self) -> bool {
        self.state.as_ref().is_some()
    }

    /// Drive the in-flight subscription wire: latch a fresh drag from an emitter glyph, a
    /// subscription pin or a trigger, track the snapped opposite end, and commit a
    /// `SetSubscription { to: true }` on release over a valid target. The context's Esc drops
    /// the wire.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, CanvasCtx<'_>>) {
        let ctx = stage.ctx;
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        // Latch a fresh drag only when idle. Distinct widget-id spaces and one press mean at most
        // one glyph starts a drag this frame, so the scan order is arbitrary.
        if self.state.is_none() {
            let latched = GlyphDrag::latch(&geometry.events, |_| true)
                .map(InFlight::Emitter)
                .or_else(|| GlyphDrag::latch(&geometry.subs, |_| true).map(InFlight::Pin))
                .or_else(|| GlyphDrag::latch(&geometry.triggers, |_| true).map(InFlight::Trigger));
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
        // A fixed end deleted under the drag drops the wire; it is not re-latched.
        if !graph_ctx.contains(state.node()) {
            return;
        }
        // Refresh the snapped opposite end, then read the source glyph's drag state off its own
        // layer: its transition out of `dragging` is the release edge.
        let released = match &mut state {
            InFlight::Emitter(drag) => {
                drag.snap = scan_listener_target(ui, ctx, drag.from);
                !drag.held(&geometry.events)
            }
            InFlight::Pin(drag) => {
                drag.snap = scan_emitter_target(ui, ctx, drag.from);
                !drag.held(&geometry.subs)
            }
            InFlight::Trigger(drag) => {
                drag.snap = scan_emitter_target(ui, ctx, drag.from.node_id);
                !drag.held(&geometry.triggers)
            }
        };
        self.state = Some(state);
        if !released {
            return;
        }
        // Released over a valid target: every direction resolves to one (emitter, listener)
        // pair and commits the same idempotent intent.
        let pair = match state {
            InFlight::Emitter(GlyphDrag {
                from,
                snap: Some(listener),
            }) => Some((from, listener)),
            _ => match state.listener() {
                Some((listener, Some(emitter))) => Some((emitter, listener)),
                _ => None,
            },
        };
        if let Some((emitter, listener)) = pair {
            stage.out.push_graph(
                stage.ctx.graph_ctx().graph(),
                GraphIntent::SetSubscription {
                    subscription: Subscription::new(emitter, listener),
                    to: true,
                },
            );
        }
        self.state = None;
    }

    /// Force the hover flag on the glyph the wire is currently snapped to, so it paints as the
    /// drop target. Palantir suppresses `response.hovered` on every widget but the drag-capture
    /// owner while a drag is live, so the snapped target would stay at its idle color.
    pub(crate) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        match self.state.as_ref() {
            Some(InFlight::Emitter(drag)) => match drag.snap {
                Some(Listener::Pin(node_id)) => geometry.subs.set_hovered(node_id),
                Some(Listener::Trigger(trigger)) => geometry.triggers.set_hovered(trigger),
                None => {}
            },
            Some(&state) => {
                if let Some((_, Some(emitter))) = state.listener() {
                    geometry.events.set_hovered(emitter);
                }
            }
            None => {}
        }
    }

    /// Paint the in-flight drag preview: a cubic between the emitter side (`p0`) and the
    /// listener side (`p3`). The end the drag started from is fixed to its glyph; the free end
    /// follows the snapped opposite glyph (when set) or the pointer. The emitter is always `p0`,
    /// so the preview keeps a committed wire's shape in either direction.
    pub(crate) fn record(&self, ui: &mut Ui, ctx: DrawCtx<'_>) {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        let Some(state) = self.state.as_ref().copied() else {
            return;
        };
        let pointer = canvas::world_pointer(ui, graph_ctx.graph(), graph_ctx.camera());
        let ends = match state {
            InFlight::Emitter(drag) => geometry.events.center(drag.from).zip(match drag.snap {
                Some(listener) => geometry.listener_center(listener),
                None => pointer,
            }),
            InFlight::Pin(drag) => drag
                .free_end(pointer, &geometry.events)
                .zip(geometry.subs.center(drag.from)),
            InFlight::Trigger(drag) => drag
                .free_end(pointer, &geometry.events)
                .zip(geometry.triggers.center(drag.from)),
        };
        let Some((p0, p3)) = ends else {
            return;
        };
        let theme = graph_ctx.theme();
        let width = theme.canvas.wire_at(ctx.zoom());
        WirePaint::from(theme.port.event).stroke(ui, &Wire::event(p0, p3), width, None);
    }
}

/// The listener under the pointer that's a valid drop for `emitter`: a pinned node's pin or a
/// trigger glyph, on a node other than the emitter's own, which a node cannot subscribe to.
fn scan_listener_target(ui: &mut Ui, ctx: CanvasCtx<'_>, emitter: EventPort) -> Option<Listener> {
    let pointer = ui.pointer_pos()?;
    let others = || {
        ctx.graph_ctx()
            .nodes()
            .filter(move |n| n.node_id != emitter.node_id)
    };
    let geometry = ctx.geometry();
    geometry
        .subs
        .first_containing(pointer, others().filter(|n| n.pinned()).map(|n| n.node_id))
        .map(Listener::Pin)
        .or_else(|| {
            geometry
                .triggers
                .first_containing(pointer, others().flat_map(NodeCtx::trigger_ports))
                .map(Listener::Trigger)
        })
}

/// Emitter event glyph under the pointer that's a valid drop for a wire dragged from a listener
/// on `listener_node`: any node's event other than that node's own. Mirror of
/// [`scan_listener_target`] for the reverse drag.
fn scan_emitter_target(
    ui: &mut Ui,
    ctx: CanvasCtx<'_>,
    listener_node: NodeId,
) -> Option<EventPort> {
    let pointer = ui.pointer_pos()?;
    let candidates = ctx
        .graph_ctx()
        .nodes()
        .filter(|n| n.node_id != listener_node)
        .flat_map(NodeCtx::event_ports);
    ctx.geometry().events.first_containing(pointer, candidates)
}

#[cfg(test)]
mod tests {
    use palantir::PointerButton;
    use scenarium::{
        EventDecl, EventPort, EventPorts, GraphId, Interface, Listener, PortId, Subgraph,
        SubgraphId, Subscription,
    };

    use crate::core::document::internals::{DocumentFixture, insert_subgraph};
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;
    use crate::gui::pane::graph_ui::wid;

    /// An instance whose interface declares a trigger shows a pin, as a sink does; an event wire
    /// pulled from a node's event lands on the pin and commits the subscription to it, and one
    /// pulled back from the pin to the event commits the same edge.
    #[test]
    fn an_event_wire_lands_on_an_instances_pin() {
        let mut fixture = DocumentFixture::probes(0);
        let emitter = fixture.node_at(glam::Vec2::new(40.0, 40.0));
        let id = SubgraphId::from_u128(1);
        let interface = Interface::default()
            .with_event_ports(EventPorts {
                events: vec![],
                trigger: Some(EventDecl::new("go")),
            })
            .unwrap();
        insert_subgraph(&mut fixture.document, id, Subgraph::new("relay", interface));
        let instance = fixture.add_instance(id);
        let mut h = CanvasHarness::new(fixture);
        h.prime(2);
        let event = EventPort::new(emitter, PortId::new(1));
        let pin = wid::subscription_glyph(GraphId::Main, instance);
        let expected = Subscription::new(event, Listener::Pin(instance));
        let committed = |intents: &[GraphIntent]| {
            matches!(
                intents,
                [GraphIntent::SetSubscription { subscription, to: true }] if *subscription == expected
            )
        };
        let pin_rect = h.ui.rect(pin).expect("the pin shows");
        let pin_at = pin_rect.center();
        // The pin straddles the node's corner, and the body takes a press on its own half.
        let pin_outside = (pin_rect.min + pin_rect.center()) * 0.5;
        h.ui.press(wid::event_glyph(GraphId::Main, event), PointerButton::Left);
        h.frame();
        h.ui.drag_to(pin_at);
        h.frame();
        h.ui.release(PointerButton::Left);
        let released = h.frame();
        assert!(committed(&released), "{released:?}");

        h.advance_past_double_click();
        let event_at =
            h.ui.rect(wid::event_glyph(GraphId::Main, event))
                .expect("the event shows")
                .center();
        h.ui.press(pin_outside, PointerButton::Left);
        h.frame();
        h.ui.drag_to(event_at);
        h.frame();
        h.ui.release(PointerButton::Left);
        let released = h.frame();
        assert!(committed(&released), "{released:?}");
    }
}
