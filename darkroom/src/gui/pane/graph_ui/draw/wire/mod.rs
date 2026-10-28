//! Everything the canvas's two wire families share.
//!
//! **The curve.** A data wire ([`crate::gui::pane::graph_ui::gesture::wire_ui`]) and
//! an event wire ([`crate::gui::pane::graph_ui::gesture::subscription_ui`]) are both one
//! [`Wire`]: two endpoints plus the interior control points its family's handle
//! rule placed, or, tied, one `Wire` for each piece between its ties
//! ([`TieLayout`]). That value is what the cull test, the breaker probe, and
//! the paint call all take, and [`WirePass::draw_wire`] runs all three in one
//! call off the per-frame inputs both renderers need — so they stay visually
//! identical apart from paint and handle shape, and can't drift.
//!
//! **The gesture.** [`GlyphDrag`](crate::gui::pane::graph_ui::gesture::glyph_drag::GlyphDrag) is one drag from a latched glyph to
//! whatever compatible glyph the pointer is over, generic over the two
//! [`PortLayer`](crate::gui::pane::graph_ui::frame::canvas_geometry::port_layer::PortLayer) key domains it
//! spans: a data wire drags `PortRef → PortRef`, an event wire drags `EventPort → NodeId` or —
//! started from the other end — `NodeId → EventPort`. Latching, the release edge, the fixed end's
//! node, and where the preview's free end sits are stated once here; each family still owns which
//! glyphs are candidates and what a release commits.

use std::ops::Range;

use glam::Vec2;

use palantir::widget::{CubicBezier, LineCap, Shape};
use palantir::{ColorRamp, RgbaF32, Stroke, Ui};

use crate::core::document::wire_key::DataWire;
use crate::core::document::wire_key::WireKey;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::frame::tie_layout::TieLayout;
use crate::gui::pane::graph_ui::frame::wire::{BEZIER_SAMPLES, Wire};
use crate::gui::pane::graph_ui::gesture::breaker_ui::breaker_probe::BreakerProbe;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_probe::TieProbe;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortRef;
use crate::gui::theme::color;
use crate::gui::widgets::hover::Hover;

/// What one wire paints with: its stroke color, and the ramp that color
/// multiplies along the curve when the two ends differ.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct WirePaint {
    color: RgbaF32,
    ramp: Option<ColorRamp>,
}

impl WirePaint {
    /// Stroke `wire` `width` wide with this paint, round-capped: the single place the wire
    /// `Shape` is built, so data and event curves cannot drift in width policy, cap, or
    /// primitive.
    pub(crate) fn stroke(self, ui: &mut Ui, wire: &Wire, width: f32) {
        let mut shape = Shape::cubic_bezier(
            CubicBezier {
                p0: wire.p0,
                p1: wire.p1,
                p2: wire.p2,
                p3: wire.p3,
            },
            Stroke::new(self.color, width),
        )
        .cap(LineCap::Round);
        if let Some(ramp) = self.ramp {
            shape = shape.ramp(ramp);
        }
        ui.add_shape(shape);
    }

    /// The part of this paint from `from` to `to` along the wire, both fractions of it: the ramp
    /// between its colors there, so a wire split into pieces paints as the whole one does.
    fn between(self, from: f32, to: f32) -> Self {
        Self {
            color: self.color,
            ramp: self
                .ramp
                .map(|ramp| ColorRamp::two_stop(ramp.color_at(from), ramp.color_at(to))),
        }
    }
}

impl From<RgbaF32> for WirePaint {
    /// One color for the whole curve.
    fn from(color: RgbaF32) -> Self {
        Self { color, ramp: None }
    }
}

/// The per-frame inputs both wire renderers need, bundled so each `draw` takes
/// one argument instead of six. Built once in
/// [`crate::gui::pane::graph_ui::GraphUi::record_canvas`] and passed by `&mut`, so the
/// breaker probe reborrows into each renderer in turn.
///
/// The canvas-wide half is [`DrawCtx`] itself, not a re-declaration of its
/// fields: the wires record in the same pass as the node bodies they run
/// between, off the same theme, graph, geometry and cull, and `WirePass` was
/// built two lines from a live `DrawCtx`. What's left here is what only a
/// wire pass has — the breaker probe it marks hits against, and this frame's
/// emphasis tier.
#[derive(Debug)]
pub(crate) struct WirePass<'a, 'p, 't> {
    /// Shared with the node-body draws — `Copy`, so it rides along by value.
    pub(crate) draw_ctx: DrawCtx<'a>,
    pub(crate) probe: &'a mut BreakerProbe<'p>,
    /// The tie line in flight, which marks the pieces it crosses.
    pub(crate) tie_line: &'a mut TieProbe<'t>,
    /// The wire a hovered entry of a tie's menu names, which paints lit as
    /// a hovered endpoint lights a wire.
    pub(crate) lit: Option<WireKey>,
    pub(crate) emphasis: &'a WireEmphasis,
    /// This frame's tied wires, which paint as pieces.
    pub(crate) ties: &'a TieLayout,
}

impl WirePass<'_, '_, '_> {
    /// A wire's width at the camera's zoom, before its emphasis tier.
    fn wire_width(&self) -> f32 {
        let theme = self.draw_ctx.theme();
        theme
            .stroke
            .wire_at(self.draw_ctx.graph_ctx().viewport().zoom)
    }

    /// Cull, breaker-probe, tier, and paint one committed wire, its `pieces`
    /// in order from its output end, reporting whether the breaker crossed
    /// it — the whole per-wire body, shared by both renderers so they can't
    /// drift in culling, emphasis, or alarm color. An untied wire is one
    /// piece; a tied one is a piece from each point to the next.
    ///
    /// A wire the cull drops is still probed, as a stroke can pan or zoom its
    /// crossings out of view; only its paint is skipped. The ends' `tint` is
    /// called only for a wire painted unbroken, so a family pays for resolving
    /// its endpoint colors only where they actually show.
    ///
    /// Recording the hit stays with the caller — only it knows the domain key
    /// to hand `probe.mark_broken_*`.
    pub(crate) fn draw_wire(
        &mut self,
        ui: &mut Ui,
        pieces: &[Wire],
        ends: WireEnds<impl FnOnce() -> WireTint>,
    ) -> bool {
        let WireEnds {
            wire: key,
            hover: endpoint,
            tint,
        } = ends;
        let cull = self.draw_ctx.cull();
        // Probed whole, off-screen pieces too: a stroke that pans or zooms mid-gesture still
        // cuts or ties what it crossed out of view. Culled is unpainted, not unprobed.
        let broken = pieces.iter().any(|piece| self.probe.crosses_wire(piece));
        let crossed = self.tie_line.cross_wire(key, pieces);
        if !pieces.iter().any(|piece| cull.keeps_rect(piece.hull())) {
            return broken;
        }
        let mark = match (broken, endpoint) {
            (true, _) => WireMark::Broken,
            (false, Hover::Over) => WireMark::EndpointHovered,
            (false, Hover::Rest) => WireMark::Plain,
        };
        let stroke = self.emphasis.stroke(self.wire_width(), mark);
        let theme = self.draw_ctx.theme();
        // A broken wire paints flat so the alarm read isn't diluted by the
        // family's own gradient, and it outranks the hover tint outright.
        let paint = if broken {
            WirePaint::from(theme.canvas.wire_broken)
        } else {
            self.emphasis.paint(tint(), stroke.tier)
        };
        for (idx, (piece, share)) in pieces.iter().zip(shares(pieces)).enumerate() {
            // The piece a tie line crosses keeps its own paint at the emphasized width, under the
            // band the line previews there.
            let crossed = crossed.is_some_and(|crossing| crossing.piece == idx);
            if !crossed && !cull.keeps_rect(piece.hull()) {
                continue;
            }
            let paint = if pieces.len() == 1 {
                paint
            } else {
                paint.between(share.start, share.end)
            };
            let width = if crossed {
                width(self.wire_width(), Tier::Emphasized)
            } else {
                stroke.width
            };
            paint.stroke(ui, piece, width);
        }
        // A cut wire also takes a form of its own, a bar across its middle, so the alarm does
        // not rest on its hue alone.
        if broken && let Some(middle) = Middle::of(pieces) {
            let half = middle.dir.perp() * (theme.canvas.cut_bar_length * 0.5);
            ui.add_shape(
                Shape::line(
                    middle.at - half,
                    middle.at + half,
                    Stroke::new(theme.canvas.wire_broken, theme.canvas.cut_bar_width),
                )
                .cap(LineCap::Round),
            );
        }
        broken
    }
}

impl WirePass<'_, '_, '_> {
    /// Paint every committed data wire on the current scene retained by the
    /// pass's cull region, marking those the active breaker crosses as broken
    /// via `probe.mark_broken_input` for the breaker's release-frame drain.
    ///
    /// Reads nothing but committed scene state, so it is the pass's rather than the
    /// in-flight gesture's.
    pub(crate) fn draw_data_wires(&mut self, ui: &mut Ui) {
        let (graph_ctx, geometry) = (self.draw_ctx.graph_ctx(), self.draw_ctx.geometry());
        for (consumer, producer) in graph_ctx.wires() {
            let (src, tgt) = (PortRef::from(producer), PortRef::from(consumer));
            let (Some(p0), Some(p3)) = (geometry.ports.center(src), geometry.ports.center(tgt))
            else {
                continue;
            };
            let key = WireKey::Data(DataWire {
                output: producer,
                input: consumer,
            });
            let hover = Hover::from(
                geometry.ports.is_hovered(src)
                    || geometry.ports.is_hovered(tgt)
                    || self.lit == Some(key)
                    || wid::lit_by_tie(ui, graph_ctx.view(), key),
            );
            let untied = [Wire::data(p0, p3)];
            let pieces = self.ties.pieces(key).unwrap_or(&untied);
            let ends = WireEnds {
                wire: key,
                hover,
                tint: || data_tint(graph_ctx, src, tgt),
            };
            if self.draw_wire(ui, pieces, ends) {
                self.probe.mark_broken_input(consumer);
            }
        }
    }

    /// Paint every committed subscription wire on the current scene retained by
    /// the pass's cull region, marking those the active breaker crosses as broken
    /// via `probe.mark_broken_subscription` for the breaker's release-frame drain.
    ///
    /// Reads nothing but committed scene state, as [`Self::draw_data_wires`] does.
    pub(crate) fn draw_event_wires(&mut self, ui: &mut Ui) {
        let (theme, graph_ctx, geometry) = (
            self.draw_ctx.theme(),
            self.draw_ctx.graph_ctx(),
            self.draw_ctx.geometry(),
        );
        for s in graph_ctx.subscriptions() {
            let (Some(p0), Some(p3)) = (
                geometry.events.center(s.event),
                geometry.subs.center(s.subscriber),
            ) else {
                continue;
            };
            let key = WireKey::Event(s);
            let hover = Hover::from(
                geometry.events.is_hovered(s.event)
                    || geometry.subs.is_hovered(s.subscriber)
                    || self.lit == Some(key)
                    || wid::lit_by_tie(ui, graph_ctx.view(), key),
            );
            let untied = [Wire::event(p0, p3)];
            let pieces = self.ties.pieces(key).unwrap_or(&untied);
            // Events carry no data type, so one neutral swatch runs the whole
            // curve rather than a per-end gradient.
            let ends = WireEnds {
                wire: key,
                hover,
                tint: || WireTint::flat(theme.port.event),
            };
            if self.draw_wire(ui, pieces, ends) {
                self.probe.mark_broken_subscription(s);
            }
        }
    }
}

/// The endpoint colors of a committed data wire: the two ports' type colors,
/// so each end matches the port it touches and the wire reads as its data type
/// (both ends share it unless the input is the untyped `Any`).
fn data_tint(graph_ctx: GraphCtx<'_>, src: PortRef, tgt: PortRef) -> WireTint {
    let theme = graph_ctx.theme();
    WireTint::new(
        theme.port_color(graph_ctx.data_type(src)),
        theme.port_color(graph_ctx.data_type(tgt)),
    )
}

/// The part of a wire's ramp each of `pieces` paints, in order from its output end: the whole
/// wire split at the piece ends by chord length, so the pieces paint the whole one's ramp. A
/// wire with no length gives each piece the whole ramp's end.
/// A wire's point halfway along its length, and the way it runs there, a unit vector.
#[derive(Clone, Copy, Debug)]
struct Middle {
    at: Vec2,
    dir: Vec2,
}

impl Middle {
    /// The middle of the curve `pieces` draw, by arc length: measured over each piece's
    /// [`BEZIER_SAMPLES`] chords, as the probes sample it, and placed back on the cubic at the
    /// chord's parameter, so the mark sits on the curve the wire paints. `None` for a wire of no
    /// length.
    fn of(pieces: &[Wire]) -> Option<Self> {
        let total: f32 = pieces
            .iter()
            .flat_map(Wire::chords)
            .map(|[a, b]| a.distance(b))
            .sum();
        let mut left = total * 0.5;
        for piece in pieces {
            for (idx, [a, b]) in piece.chords().enumerate() {
                let length = a.distance(b);
                if length > 0.0 && left <= length {
                    let t = (idx as f32 + left / length) / BEZIER_SAMPLES as f32;
                    return Some(Self {
                        at: piece.at(t),
                        dir: piece.velocity(t).normalize_or((b - a) / length),
                    });
                }
                left -= length;
            }
        }
        None
    }
}

fn shares(pieces: &[Wire]) -> impl ExactSizeIterator<Item = Range<f32>> + '_ {
    let total: f32 = pieces.iter().map(Wire::chord).sum();
    let mut from = 0.0;
    pieces.iter().map(move |piece| {
        let to = if total > 0.0 {
            (from + piece.chord() / total).min(1.0)
        } else {
            1.0
        };
        let share = from..to;
        from = to;
        share
    })
}

/// What a committed wire's ends bring to its paint: the wire they bound,
/// whether an endpoint glyph or a tie on it is hovered, and `tint`, which
/// resolves the colors the ends paint it with.
#[derive(Debug)]
pub(crate) struct WireEnds<F> {
    pub(crate) wire: WireKey,
    pub(crate) hover: Hover,
    pub(crate) tint: F,
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
    pub(crate) const fn new(start: RgbaF32, end: RgbaF32) -> Self {
        Self { start, end }
    }

    /// One color for the whole curve — an event wire (events carry no data
    /// type), or a data wire whose type mismatch paints it all in the warning
    /// color.
    pub(crate) const fn flat(color: RgbaF32) -> Self {
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

/// Alpha of the standing wires while a wire gesture (new-wire drag,
/// subscription drag, breaker scribble) is active — dimming the plumbing so
/// the preview, candidate ports, and broken-alarm wires pop.
const WIRE_DRAG_FADE: f32 = 0.35;

/// Width multiplier for an emphasized (hovered or broken-alarm) wire, so
/// one wire stays traceable through a crossing.
const WIRE_HOVER_WIDTH: f32 = 1.25;

/// Whether any wire gesture — a new-wire drag, a subscription drag, a
/// breaker scribble — is in flight this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WireSet {
    /// No gesture: the standing set paints at its rest tier.
    Standing,
    /// A gesture fades the standing set so the preview, candidate ports, and
    /// broken-alarm wires pop.
    Faded,
}

/// What marks one wire this frame, before the pass's tier applies. A broken
/// wire never *also* reads as hovered: the alarm hue wins outright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireMark {
    Plain,
    /// An endpoint glyph is hovered.
    EndpointHovered,
    /// The active breaker crosses it.
    Broken,
}

/// The tier a wire's paint runs through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    /// The rest-dim tint (or the fade, while a gesture is in flight) at base
    /// width.
    Rest,
    /// Full-strength color and the width lift.
    Emphasized,
}

/// Per-pass emphasis state shared by every wire renderer, resolved once in
/// the canvas frame. The tiers: while any wire gesture is in flight
/// ([`WireSet::Faded`]) the standing set drops to [`WIRE_DRAG_FADE`] alpha and hover
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
    set: WireSet,
    canvas_bg: RgbaF32,
}

impl WireEmphasis {
    /// Resolve this frame's emphasis inputs. `set` is faded while any wire
    /// gesture is active — the callers OR together the two drag controllers
    /// and the breaker.
    pub(crate) const fn resolve(canvas_bg: RgbaF32, set: WireSet) -> Self {
        Self { set, canvas_bg }
    }

    /// This frame's paint tier and width for one wire marked `mark`: a broken
    /// wire paints at its rest tier — the caller takes the flat alarm color
    /// instead — and, like a hovered one, takes the width lift.
    fn stroke(&self, base_width: f32, mark: WireMark) -> WireStroke {
        let tier = match mark {
            WireMark::EndpointHovered if self.set == WireSet::Standing => Tier::Emphasized,
            WireMark::Plain | WireMark::EndpointHovered | WireMark::Broken => Tier::Rest,
        };
        let lifted = match mark {
            WireMark::Broken => Tier::Emphasized,
            WireMark::Plain | WireMark::EndpointHovered => tier,
        };
        WireStroke {
            tier,
            width: width(base_width, lifted),
        }
    }

    /// The paint for a non-broken wire: `tint`'s endpoint colors run
    /// through this frame's tier. Distinct ends ramp `p0` → `p3` over a
    /// white stroke, so the ramp's colors are the ones that show. Equal
    /// ends paint flat rather than ramping between two identical stops.
    fn paint(&self, tint: WireTint, tier: Tier) -> WirePaint {
        let start = self.tint(tint.start, tier);
        let end = self.tint(tint.end, tier);
        if start == end {
            return WirePaint::from(start);
        }
        WirePaint {
            color: RgbaF32::WHITE,
            ramp: Some(ColorRamp::two_stop(start, end)),
        }
    }

    /// The tiered color for a (non-broken) wire endpoint. While a gesture
    /// fades the set the fade wins over the tier — the snap target's forced
    /// endpoint hover must not re-emphasize a faded wire.
    const fn tint(&self, c: RgbaF32, tier: Tier) -> RgbaF32 {
        match (self.set, tier) {
            (WireSet::Faded, _) => c.with_alpha(WIRE_DRAG_FADE),
            (WireSet::Standing, Tier::Emphasized) => c,
            (WireSet::Standing, Tier::Rest) => color::toward(c, self.canvas_bg, WIRE_REST_DIM),
        }
    }
}

/// The tiered stroke width. Broken-alarm wires take the emphasized width
/// too: full width against the faded rest of the set is the alarm.
fn width(base: f32, tier: Tier) -> f32 {
    match tier {
        Tier::Emphasized => base * WIRE_HOVER_WIDTH,
        Tier::Rest => base,
    }
}

/// How one wire paints this frame, as resolved by [`WireEmphasis::stroke`].
/// The breaker's verdict isn't a field: [`WirePass::draw_wire`] hands it
/// straight back to the caller, the only party that can record the hit.
#[derive(Clone, Copy, Debug)]
struct WireStroke {
    /// [`Tier::Emphasized`] paints at full strength rather than the rest-dim
    /// tint. Never for a broken wire — the alarm read wins outright.
    tier: Tier,
    width: f32,
}

#[cfg(test)]
mod tests;
