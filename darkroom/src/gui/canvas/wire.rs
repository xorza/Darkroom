//! Shared connection-curve primitives. Both the data-wire renderer
//! ([`super::connection_ui`]) and the event/subscription-wire renderer
//! ([`super::event_connection_ui`]) compute their own control handles but
//! draw the curve through the one primitive here, so the two stay visually
//! identical apart from their brush.

use glam::Vec2;
use palantir::{Brush, LineCap, Shape, Ui};

/// Minimum length of a wire's bezier control handles, so a short or backward
/// link still bows out into a readable curve.
pub(crate) const MIN_HANDLE: f32 = 40.0;

/// The two interior control points of a connection cubic — the named result
/// of each renderer's handle-placement function.
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
