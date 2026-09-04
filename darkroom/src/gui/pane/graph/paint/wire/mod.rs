//! Everything the canvas's two wire families share.
//!
//! **The curve.** A data connection ([`crate::gui::pane::graph::gesture::connection`]) and
//! an event wire ([`crate::gui::pane::graph::gesture::subscription`]) are both one
//! [`Wire`]: two endpoints plus the interior control points its family's handle
//! rule placed. That single value is what the cull test, the breaker probe, and
//! the paint call all take, and [`WirePass::draw_wire`] runs all three in one
//! call off the per-frame inputs both renderers need — so they stay visually
//! identical apart from brush and handle shape, and can't drift.
//!
//! **The gesture.** [`GlyphDrag`] is one drag from a latched glyph to whatever
//! compatible glyph the pointer is over, generic over the two
//! [`PortLayer`] key domains it spans: a data connection drags
//! `PortRef → PortRef`, an event wire drags `EventRef → NodeId` or — started
//! from the other end — `NodeId → EventRef`. Latching, the release edge, which
//! pane owns the gesture, and where the preview's free end sits are stated once
//! here; each family still owns which glyphs are candidates and what a release
//! commits.

use glam::Vec2;
use palantir::{RgbaF32, CurveBrush, LineCap, LinearGradient, Rect, Shape, Size, Stop, Ui};
use scenarium::NodeId;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::frame::geometry::{GlyphKey, PortLayer};
use crate::gui::pane::graph::gesture::breaker::BreakerProbe;
use crate::gui::theme::color::toward;

/// Minimum length of a wire's bezier control handles, so a short or backward
/// link still bows out into a readable curve.
const MIN_HANDLE: f32 = 30.0;

/// Upper bound on the *vertical-gap* term of [`Wire::data`]'s handle
/// length, so a tall forward span bows into a gentle S rather than a huge loop.
const MAX_HANDLE: f32 = 120.0;

/// Gain on [`Wire::data`]'s *backward-reach* term: `reach = BACKREACH_GAIN * sqrt(distance)`.
/// A square-root law (not linear, not a fixed cap) so the loop keeps growing as the far end
/// moves further left — a flat cap reads short across a big gap — yet grows ever more slowly,
/// so it never sprawls out to the sides the way a linear reach does. Tuned so a node-width
/// backlink (~180px) reaches ~135px.
const BACKREACH_GAIN: f32 = 10.0;

/// One wire's full cubic: the endpoints `p0` → `p3` plus the interior
/// control points its handle rule placed. Built through [`Wire::data`] or
/// [`Wire::event`], so the choice of rule happens once — at the only place
/// that knows which family the curve belongs to — and every consumer
/// downstream (cull, breaker, paint) takes the finished curve.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Wire {
    pub(crate) p0: Vec2,
    pub(crate) p1: Vec2,
    pub(crate) p2: Vec2,
    pub(crate) p3: Vec2,
}

impl Wire {
    /// A left-to-right cubic between `p0` (the output port) and `p3` (the
    /// input port): both handles run horizontally so the curve leaves `p0`
    /// rightward and arrives at `p3` leftward. Shared by the permanent and
    /// in-flight draws so a preview always matches its eventual committed
    /// curve exactly.
    ///
    /// The handle length is the larger of two terms:
    /// - **Forward** — half the *vertical* gap (clamped to `[MIN_HANDLE,
    ///   MAX_HANDLE]`): near-level anchors stay taut, stacked ones bow into a
    ///   gentle S without over-looping on a tall span.
    /// - **Backward** — when `p3` sits *left* of `p0` the curve must double back
    ///   on itself. A short handle whips it straight across whatever sits between
    ///   (the "hidden under the node" look); reaching out by `BACKREACH_GAIN *
    ///   sqrt(distance)` instead bows both ends into one wide, smooth loop that
    ///   leaves `p0` rightward, arcs around, and re-enters `p3` leftward. The
    ///   `sqrt` keeps the loop scaling with the backward distance (so a far-away
    ///   `p3` still gets a proper loop, not a stub) while growing slowly enough
    ///   that it never sprawls out to the sides.
    pub(crate) fn data(p0: Vec2, p3: Vec2) -> Self {
        let vertical = ((p3.y - p0.y).abs() * 0.5).clamp(MIN_HANDLE, MAX_HANDLE);
        let backreach = BACKREACH_GAIN * (p0.x - p3.x).max(0.0).sqrt();
        let len = vertical.max(backreach);
        Self {
            p0,
            p1: p0 + Vec2::new(len, 0.0),
            p2: p3 - Vec2::new(len, 0.0),
            p3,
        }
    }

    /// An event wire from emitter `p0` (a triangle on the right of its node)
    /// to subscriber pin `p3` (the top-left pin). The emitter handle leaves
    /// rightward like a data output; the subscriber handle points
    /// **up-left**, matching the pin's outward-pointing triangle so the wire
    /// meets it head-on.
    pub(crate) fn event(p0: Vec2, p3: Vec2) -> Self {
        let d = (p0.distance(p3) * 0.4).max(MIN_HANDLE);
        // (-1, -1) is up-left in screen space (y grows downward).
        let up_left = Vec2::new(-1.0, -1.0).normalize();
        Self {
            p0,
            p1: p0 + Vec2::new(d, 0.0),
            p2: p3 + up_left * d,
            p3,
        }
    }

    /// The curve's control-point bounding box. A cubic stays inside its
    /// control hull, so this is a conservative bound — what
    /// [`CullRegion::keeps_wire`](crate::gui::pane::graph::frame::cull::CullRegion::keeps_wire)
    /// tests against.
    pub(crate) fn hull(&self) -> Rect {
        let min = self.p0.min(self.p1).min(self.p2).min(self.p3);
        let max = self.p0.max(self.p1).max(self.p2).max(self.p3);
        Rect {
            min,
            size: Size::new(max.x - min.x, max.y - min.y),
        }
    }

    /// Emit the stroked curve (round caps). The single place the wire
    /// `Shape` is built, so data and event curves can't drift in width
    /// policy, cap, or primitive.
    pub(crate) fn add(&self, ui: &mut Ui, width: f32, brush: impl Into<CurveBrush>) {
        ui.add_shape(
            Shape::cubic_bezier(self.p0, self.p1, self.p2, self.p3, width)
                .brush(brush)
                .cap(LineCap::Round),
        );
    }
}

/// One in-flight wire drag: the glyph the press latched (`from`, a key in
/// layer `A`) and the compatible glyph currently under the pointer (`snap`, a
/// key in layer `B`).
///
/// Identity-only — both ends resolve their position out of `CanvasGeometry`
/// every frame, so a drag survives node moves and relayouts. The direction of
/// an event drag flips the two domains, which is why `A` and `B` are separate
/// parameters rather than one: a subscriber-started drag is a
/// `GlyphDrag<NodeId, EventRef>` and an emitter-started one the reverse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GlyphDrag<A, B> {
    /// The glyph the press latched. Fixed for the drag's whole life.
    pub(crate) from: A,
    /// Compatible glyph currently under the pointer — the preview's snap end,
    /// the forced hover highlight, and what a release commits against.
    pub(crate) snap: Option<B>,
}

impl<A: GlyphKey, B: GlyphKey> GlyphDrag<A, B> {
    /// A drag off `from` with nothing snapped yet.
    pub(crate) fn new(from: A) -> Self {
        Self { from, snap: None }
    }

    /// Latch on the first of `keys` whose glyph began a drag this frame, or
    /// `None` when no press landed on one. Candidate order is the caller's
    /// tie-break — the topmost recorded glyph comes first.
    pub(crate) fn latch(layer: &PortLayer<A>, keys: impl Iterator<Item = A>) -> Option<Self> {
        layer.first_drag_started(keys).map(Self::new)
    }

    /// The node the fixed end hangs off: the pane that owns the gesture, and
    /// the node whose disappearance (undo, a breaker swipe) ends it.
    pub(crate) fn node(self) -> NodeId {
        self.from.node()
    }

    /// Whether the press that latched this drag is still held.
    /// `PortLayer::dragging` rolls up `drag_delta().is_some() ||
    /// drag_started()`, so its transition to `false` is the release edge.
    pub(crate) fn held(self, layer: &PortLayer<A>) -> bool {
        layer.dragging(self.from)
    }

    /// The moving end of the preview curve: the snapped glyph's center once
    /// the drag has a target, else the bare pointer in canvas-world coords.
    /// `None` on a frame where neither resolves (pointer off-window, or a snap
    /// target that hasn't measured yet) — the preview simply doesn't paint
    /// that frame. `canvas_origin` is the inner canvas's pre-transform origin.
    pub(crate) fn free_end(
        self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        canvas_origin: Vec2,
        layer: &PortLayer<B>,
    ) -> Option<Vec2> {
        match self.snap {
            Some(key) => layer.center(key),
            None => ui
                .pointer_pos()
                .map(|p| graph_ctx.viewport().to_world(p - canvas_origin)),
        }
    }
}

/// The per-frame inputs both wire renderers need, bundled so each `draw` takes
/// one argument instead of six. Built once in
/// [`crate::gui::pane::graph::GraphUI::record_canvas`] and passed by `&mut`, so the
/// breaker probe reborrows into each renderer in turn.
///
/// The pane-wide half is [`DrawCtx`] itself, not a re-declaration of its
/// fields: the wires record in the same pass as the node bodies they run
/// between, off the same theme, pane, geometry and cull, and `WirePass` was
/// built two lines from a live `DrawCtx`. What's left here is what only a
/// wire pass has — the breaker probe it marks hits against, and this frame's
/// emphasis tier.
#[derive(Debug)]
pub(crate) struct WirePass<'a, 'p> {
    /// Shared with the node-body draws — `Copy`, so it rides along by value.
    pub(crate) dcx: DrawCtx<'a>,
    pub(crate) probe: &'a mut BreakerProbe<'p>,
    pub(crate) emphasis: &'a WireEmphasis,
}

impl WirePass<'_, '_> {
    /// Cull, breaker-probe, brush, and paint one committed wire, reporting
    /// whether the breaker crossed it — the whole per-wire body, shared by both
    /// renderers so they can't drift in culling, emphasis, or alarm color.
    ///
    /// A wire the cull drops is not probed either: the scribble is always
    /// on-screen, so it can't have crossed an off-screen curve. `tint` is
    /// called only for a wire that survives *both* the cull and the breaker,
    /// so a family pays for resolving its endpoint colors only where they
    /// actually show.
    ///
    /// Recording the hit stays with the caller — only it knows the domain key
    /// to hand `probe.mark_broken_*`.
    pub(crate) fn draw_wire(
        &mut self,
        ui: &mut Ui,
        wire: &Wire,
        endpoint_hover: bool,
        tint: impl FnOnce() -> WireTint,
    ) -> bool {
        if !self.dcx.cull().keeps_wire(wire) {
            return false;
        }
        let broken = self.probe.crosses_wire(wire);
        let stroke = self
            .emphasis
            .stroke(self.dcx.theme().stroke_width, broken, endpoint_hover);
        // A broken wire paints flat so the alarm read isn't diluted by the
        // family's own gradient, and it outranks the hover tint outright.
        let brush = if broken {
            CurveBrush::from(self.dcx.theme().colors.connection_broken)
        } else {
            self.emphasis.brush(tint(), stroke.hovered)
        };
        wire.add(ui, stroke.width, brush);
        broken
    }
}

/// The endpoint colors a family paints a wire with *before* this frame's
/// emphasis tier is applied: `start` at the curve's `p0`, `end` at its `p3`,
/// so each end of a wire visually matches the glyph it touches.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct WireTint {
    start: RgbaF32,
    end: RgbaF32,
}

impl WireTint {
    /// Distinct colors per end, which lower to a gradient along the curve.
    pub(crate) fn new(start: RgbaF32, end: RgbaF32) -> Self {
        Self { start, end }
    }

    /// One color for the whole curve — an event wire (events carry no data
    /// type), or a data wire whose type mismatch paints it all in the warning
    /// color.
    pub(crate) fn flat(color: RgbaF32) -> Self {
        Self {
            start: color,
            end: color,
        }
    }
}

/// How far rest-state wire endpoint colors pull toward the canvas, so the
/// port dots (identity) stay the brightest points on the data path and long
/// wires don't outshine them.
const WIRE_REST_DIM: f32 = 0.15;

/// Alpha of the standing wires while a wire gesture (new-connection drag,
/// subscription drag, breaker scribble) is active — dimming the plumbing so
/// the preview, candidate ports, and broken-alarm wires pop.
const WIRE_DRAG_FADE: f32 = 0.35;

/// Width multiplier for an emphasized (hovered or broken-alarm) wire, so
/// one connection stays traceable through a crossing.
const WIRE_HOVER_WIDTH: f32 = 1.25;

/// Per-pass emphasis state shared by every wire renderer, resolved once in
/// the canvas frame. The tiers: while any wire gesture is in flight
/// (`fading`) the standing set drops to [`WIRE_DRAG_FADE`] alpha and hover
/// is off; at rest, endpoint colors pull toward the canvas; a hovered (or
/// broken-alarm) wire gets full strength and a width lift.
///
/// Emphasis is driven by endpoint *hover targets* only (port circles,
/// event glyphs, subscription pins — all with generously scaled hit
/// boxes), never by raw pointer proximity to the curve: hover-target
/// state repaints exactly when it changes, whereas pointer-derived
/// paint needs a `MOVE` subscription (a record per mouse move) to stay
/// fresh on screen.
#[derive(Debug)]
pub(crate) struct WireEmphasis {
    fading: bool,
    canvas_bg: RgbaF32,
}

impl WireEmphasis {
    /// Resolve this frame's emphasis inputs. `fading` is "any wire gesture
    /// is active" — the callers OR together the two drag controllers and
    /// the breaker.
    pub(crate) fn resolve(canvas_bg: RgbaF32, fading: bool) -> Self {
        Self { fading, canvas_bg }
    }

    /// This frame's paint tier for one wire, folding the two rules both
    /// renderers need: a broken wire never *also* reads
    /// as hovered (the alarm hue wins outright), and either state takes the
    /// width lift.
    fn stroke(&self, base_width: f32, broken: bool, endpoint_hover: bool) -> WireStroke {
        let hovered = !broken && self.hovered(endpoint_hover);
        WireStroke {
            hovered,
            width: self.width(base_width, hovered || broken),
        }
    }

    /// The brush for a non-broken wire: `tint`'s endpoint colors run through
    /// this frame's tier. Equal ends lower to a flat brush rather than a
    /// gradient between two identical stops.
    fn brush(&self, tint: WireTint, emphasized: bool) -> CurveBrush {
        let start = self.tint(tint.start, emphasized);
        let end = self.tint(tint.end, emphasized);
        if start == end {
            return CurveBrush::from(start);
        }
        // Palantir's cubic-curve lowering samples a linear-gradient brush
        // along the curve parameter `t` and ignores `angle`, so we pass 0.0
        // and the gradient runs `p0` → `p3` whichever way the curve points.
        CurveBrush::from(LinearGradient::new(
            0.0,
            [Stop::new(0.0, start), Stop::new(1.0, end)],
        ))
    }

    /// Whether this wire is hover-emphasized: an endpoint glyph is
    /// hovered. Never while a gesture fades the set — the snap target's
    /// forced endpoint hover must not re-emphasize a faded wire.
    fn hovered(&self, endpoint_hovered: bool) -> bool {
        !self.fading && endpoint_hovered
    }

    /// The tiered color for a (non-broken) wire endpoint.
    fn tint(&self, c: RgbaF32, emphasized: bool) -> RgbaF32 {
        if self.fading {
            c.with_alpha(WIRE_DRAG_FADE)
        } else if emphasized {
            c
        } else {
            toward(c, self.canvas_bg, WIRE_REST_DIM)
        }
    }

    /// The tiered stroke width. Broken-alarm wires pass `emphasized: true`
    /// too: full width against the faded rest of the set is the alarm.
    fn width(&self, base: f32, emphasized: bool) -> f32 {
        if emphasized {
            base * WIRE_HOVER_WIDTH
        } else {
            base
        }
    }
}

/// How one wire paints this frame, as resolved by [`WireEmphasis::stroke`].
/// The breaker's verdict isn't a field: [`WirePass::draw_wire`] hands it
/// straight back to the caller, the only party that can record the hit.
#[derive(Clone, Copy, Debug)]
struct WireStroke {
    /// Paints at full strength rather than the rest-dim tint. Never set for a
    /// broken wire — the alarm read wins outright.
    hovered: bool,
    width: f32,
}

#[cfg(test)]
mod tests;
