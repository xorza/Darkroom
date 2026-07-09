//! Shared connection-curve primitives. Both the data-wire renderer
//! ([`super::connection_ui`]) and the event/subscription-wire renderer
//! ([`super::subscription_ui`]) compute their own control handles but
//! draw the curve through the one primitive here — and share one emphasis
//! system (rest-dim / hover-lift / drag-fade constants + helpers) — so the
//! two stay visually identical apart from their brush.

use aperture::{Brush, Color, LineCap, Shape, Ui};
use glam::Vec2;

use crate::gui::canvas::breaker::cubic_point;

/// Minimum length of a wire's bezier control handles, so a short or backward
/// link still bows out into a readable curve.
pub(crate) const MIN_HANDLE: f32 = 30.0;

/// How far rest-state wire endpoint colors pull toward the canvas, so the
/// port dots (identity) stay the brightest points on the data path and long
/// wires don't outshine them.
pub(crate) const WIRE_REST_DIM: f32 = 0.15;

/// Alpha of the standing wires while a new one is being dragged — dimming
/// the plumbing so the preview and the candidate ports pop.
pub(crate) const WIRE_DRAG_FADE: f32 = 0.35;

/// Screen-px radius around the pointer that counts as "on the wire" for the
/// hover highlight (divided by zoom into world units).
pub(crate) const WIRE_HOVER_RADIUS: f32 = 6.0;

/// Width multiplier for a hovered wire, so one connection stays traceable
/// through a crossing.
pub(crate) const WIRE_HOVER_WIDTH: f32 = 1.25;

/// Linear-space pull of `c` toward `to` by `t`, alpha untouched. Storage
/// colors are already linear, so a straight component lerp is correct.
pub(crate) fn toward(c: Color, to: Color, t: f32) -> Color {
    Color::linear_rgba(
        c.r + (to.r - c.r) * t,
        c.g + (to.g - c.g) * t,
        c.b + (to.b - c.b) * t,
        c.a,
    )
}

/// Whether `point` lies within `radius` of the cubic — sampled, like the
/// breaker's crossing test, which is plenty for a hover threshold a few
/// pixels wide.
pub(crate) fn near_cubic(
    p0: Vec2,
    handles: &CubicHandles,
    p3: Vec2,
    point: Vec2,
    radius: f32,
) -> bool {
    const SAMPLES: u32 = 16;
    let r2 = radius * radius;
    (0..=SAMPLES).any(|i| {
        let t = i as f32 / SAMPLES as f32;
        cubic_point(p0, handles.p1, handles.p2, p3, t).distance_squared(point) <= r2
    })
}

/// The two interior control points of a connection cubic — the named result
/// of each renderer's handle-placement function.
#[derive(Debug)]
pub(crate) struct CubicHandles {
    pub(crate) p1: Vec2,
    pub(crate) p2: Vec2,
}

/// Emit a stroked cubic-bezier wire (round caps) from `p0` to `p3` through
/// `handles`. The single place the wire `Shape` is built, so data and event
/// wires can't drift in width policy, cap, or primitive.
pub(crate) fn add_cubic_wire(
    ui: &mut Ui,
    p0: Vec2,
    p3: Vec2,
    handles: CubicHandles,
    width: f32,
    brush: Brush,
) {
    ui.add_shape(Shape::CubicBezier {
        p0,
        p1: handles.p1,
        p2: handles.p2,
        p3,
        width,
        brush,
        cap: LineCap::Round,
    });
}
