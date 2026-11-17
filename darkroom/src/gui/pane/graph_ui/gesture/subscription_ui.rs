use palantir::Ui;
use scenarium::{EventPort, NodeId, Subscription, TriggerPort};

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

/// Owns the in-flight event wire, from an event or from a trigger. One wire at a time, so a
/// single `Option` suffices. The committed wires live on the authoring graph and are painted off
/// it by
/// [`WirePass::draw_event_wires`](crate::gui::pane::graph_ui::draw::wire::WirePass::draw_event_wires).
///
/// A sibling of [`crate::gui::pane::graph_ui::gesture::wire_ui::WireUi`] rather than a mode of
/// it: an event wire carries no data type, runs no cycle or const checks, and joins an event
/// glyph to a trigger glyph. What the two share (latching, the release edge, the fixed end's
/// node, the preview's free end) is [`GlyphDrag`]'s. The drag can start from either end: pull
/// from an event and drop on a trigger, or pull from a trigger and drop on an event. Held-drag
/// only.
#[derive(Default, Debug)]
pub(crate) struct SubscriptionUi {
    state: Option<InFlight>,
}

/// The in-flight event wire, discriminated by the glyph it started from, which flips the drag's
/// glyph domains. Each commits the same `SetSubscription { to: true }`.
#[derive(Clone, Copy, Debug)]
enum InFlight {
    /// Started on an event glyph; snapping to a trigger.
    Event(GlyphDrag<EventPort, TriggerPort>),
    /// Started on a trigger glyph; snapping to an event.
    Trigger(GlyphDrag<TriggerPort, EventPort>),
}

impl InFlight {
    /// The node at the drag's fixed end: whichever glyph the press latched.
    fn node(self) -> NodeId {
        match self {
            Self::Event(drag) => drag.node(),
            Self::Trigger(drag) => drag.node(),
        }
    }

    /// The subscription the drag would commit, once its free end has snapped.
    const fn subscription(self) -> Option<Subscription> {
        match self {
            Self::Event(GlyphDrag {
                from,
                snap: Some(trigger),
            }) => Some(Subscription {
                event: from,
                trigger,
            }),
            Self::Trigger(GlyphDrag {
                from,
                snap: Some(event),
            }) => Some(Subscription {
                event,
                trigger: from,
            }),
            Self::Event(_) | Self::Trigger(_) => None,
        }
    }
}

impl SubscriptionUi {
    /// Drop the in-flight event wire.
    pub(crate) const fn reset(&mut self) {
        self.state = None;
    }

    /// Whether an event wire is being dragged, which answers both the Esc policy and the
    /// wire-fade tier.
    pub(crate) const fn in_flight(&self) -> bool {
        self.state.as_ref().is_some()
    }

    /// Drive the in-flight event wire: latch a fresh drag from an event or a trigger glyph, track
    /// the snapped opposite end, and commit a `SetSubscription { to: true }` on release over a
    /// valid target. The context's Esc drops the wire.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, CanvasCtx<'_>>) {
        let ctx = stage.ctx;
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        // Latch a fresh drag only when idle. Distinct widget-id spaces and one press mean at most
        // one glyph starts a drag this frame, so the scan order is arbitrary.
        if self.state.is_none() {
            let latched = GlyphDrag::latch(&geometry.events, |_| true)
                .map(InFlight::Event)
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
            InFlight::Event(drag) => {
                drag.snap = scan_trigger_target(ui, ctx, drag.from);
                !drag.held(&geometry.events)
            }
            InFlight::Trigger(drag) => {
                drag.snap = scan_event_target(ui, ctx, drag.from.node_id);
                !drag.held(&geometry.triggers)
            }
        };
        self.state = Some(state);
        if !released {
            return;
        }
        // Released over a valid target: either direction resolves to one subscription and commits
        // the same idempotent intent.
        if let Some(subscription) = state.subscription() {
            stage.out.push_graph(
                stage.ctx.graph_ctx().graph(),
                GraphIntent::SetSubscription {
                    subscription,
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
        match self.state {
            Some(InFlight::Event(GlyphDrag {
                snap: Some(trigger),
                ..
            })) => geometry.triggers.set_hovered(trigger),
            Some(InFlight::Trigger(GlyphDrag {
                snap: Some(event), ..
            })) => geometry.events.set_hovered(event),
            Some(InFlight::Event(_) | InFlight::Trigger(_)) | None => {}
        }
    }

    /// Paint the in-flight drag preview: a cubic between the event side (`p0`) and the trigger
    /// side (`p3`). The end the drag started from is fixed to its glyph; the free end follows the
    /// snapped opposite glyph (when set) or the pointer. The event is always `p0`, so the preview
    /// keeps a committed wire's shape in either direction.
    pub(crate) fn record(&self, ui: &mut Ui, ctx: DrawCtx<'_>) {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        let Some(state) = self.state.as_ref().copied() else {
            return;
        };
        let pointer = canvas::world_pointer(ui, graph_ctx.graph(), graph_ctx.camera());
        let ends = match state {
            InFlight::Event(drag) => geometry
                .events
                .center(drag.from)
                .zip(drag.free_end(pointer, &geometry.triggers)),
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

/// The trigger under the pointer that's a valid drop for `event`: a trigger glyph on a node other
/// than the event's own, which a node cannot subscribe to.
fn scan_trigger_target(ui: &mut Ui, ctx: CanvasCtx<'_>, event: EventPort) -> Option<TriggerPort> {
    let pointer = ui.pointer_pos()?;
    let candidates = ctx
        .graph_ctx()
        .nodes()
        .filter(|n| n.node_id != event.node_id)
        .flat_map(NodeCtx::trigger_ports);
    ctx.geometry()
        .triggers
        .first_containing(pointer, candidates)
}

/// The event glyph under the pointer that's a valid drop for a wire dragged from a trigger on
/// `subscriber`: any node's event other than that node's own. Mirror of [`scan_trigger_target`].
fn scan_event_target(ui: &mut Ui, ctx: CanvasCtx<'_>, subscriber: NodeId) -> Option<EventPort> {
    let pointer = ui.pointer_pos()?;
    let candidates = ctx
        .graph_ctx()
        .nodes()
        .filter(|n| n.node_id != subscriber)
        .flat_map(NodeCtx::event_ports);
    ctx.geometry().events.first_containing(pointer, candidates)
}

#[cfg(test)]
mod tests {
    use palantir::PointerButton;
    use scenarium::{
        Compound, CompoundId, EventDecl, EventPort, EventPorts, GraphId, Interface, PortId,
        Subscription, TriggerPort,
    };

    use crate::core::document::internals::{DocumentFixture, insert_compound};
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;
    use crate::gui::pane::graph_ui::wid;

    /// An instance whose interface declares a trigger shows it at its corner, as a sink does; an
    /// event wire pulled from a node's event lands on the trigger and commits the subscription to
    /// it, and one pulled back from the trigger to the event commits the same edge.
    #[test]
    fn an_event_wire_lands_on_an_instances_trigger() {
        let mut fixture = DocumentFixture::probes(0);
        let emitter = fixture.node_at(glam::Vec2::new(40.0, 40.0));
        let id = CompoundId::from_u128(1);
        let interface = Interface::default()
            .with_event_ports(EventPorts {
                events: vec![],
                trigger: Some(EventDecl::new("go")),
            })
            .unwrap();
        insert_compound(&mut fixture.document, id, Compound::new("relay", interface));
        let instance = fixture.add_instance(id);
        let mut h = CanvasHarness::new(fixture);
        h.prime(2);
        let event = EventPort::new(emitter, PortId::new(1));
        let trigger = wid::trigger_glyph(GraphId::Main, TriggerPort::from(instance));
        let expected = Subscription::new(event, instance);
        let committed = |intents: &[GraphIntent]| {
            matches!(
                intents,
                [GraphIntent::SetSubscription { subscription, to: true }] if *subscription == expected
            )
        };
        let trigger_rect = h.ui.rect(trigger).expect("the trigger shows");
        let trigger_at = trigger_rect.center();
        // The trigger straddles the node's corner, and the body takes a press on its own half.
        let trigger_outside = (trigger_rect.min + trigger_rect.center()) * 0.5;
        h.ui.press(wid::event_glyph(GraphId::Main, event), PointerButton::Left);
        h.frame();
        h.ui.drag_to(trigger_at);
        h.frame();
        h.ui.release(PointerButton::Left);
        let released = h.frame();
        assert!(committed(&released), "{released:?}");

        h.advance_past_double_click();
        let event_at =
            h.ui.rect(wid::event_glyph(GraphId::Main, event))
                .expect("the event shows")
                .center();
        h.ui.press(trigger_outside, PointerButton::Left);
        h.frame();
        h.ui.drag_to(event_at);
        h.frame();
        h.ui.release(PointerButton::Left);
        let released = h.frame();
        assert!(committed(&released), "{released:?}");
    }
}
