//! `RgbaF32` math shared across the GUI's *derived* colors. The theme names the
//! resting colors; the places that blend one — a wire endpoint pulled toward
//! the canvas at rest, a port circle pulled toward white on hover —
//! go through here, so two emphases of the same swatch can't drift apart.

use palantir::RgbaF32;

/// Pull `c`'s color toward `to` by `t`, a straight blend of its channels, **keeping `c`'s own
/// alpha**.
///
/// Straight, not `Animatable::lerp`, which blends premultiplied and so weighs each end's hue by
/// its alpha: the alpha here is not blended, so neither end's may tilt the hue. Opacity on the
/// canvas is owned by a separate rule — the wire set drops to a fixed alpha while a gesture is in
/// flight — so a tier that shifts a *color* must leave it alone, or the two rules multiply and a
/// dimmed wire fades twice.
pub(crate) const fn toward(c: RgbaF32, to: RgbaF32, t: f32) -> RgbaF32 {
    RgbaF32::new(
        c.r + (to.r - c.r) * t,
        c.g + (to.g - c.g) * t,
        c.b + (to.b - c.b) * t,
        c.a,
    )
}

/// The hover emphasis every port shares: blend toward white, so the port lifts off the canvas.
///
/// The palette cannot carry a lifted color beside each resting one — most port colors already
/// sit on its brightest tint, which has nothing above it — so the lift is computed here instead.
pub(crate) const fn hover_lift(c: RgbaF32) -> RgbaF32 {
    const T: f32 = 0.28;
    toward(c, RgbaF32::WHITE, T)
}

#[cfg(test)]
mod tests;
