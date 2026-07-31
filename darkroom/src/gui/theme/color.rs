//! Color math shared across the GUI's *derived* swatches. The theme names the
//! resting colors; the places that blend one — a wire endpoint pulled toward
//! the canvas at rest, a port circle pulled toward white or black on hover —
//! go through here, so two emphases of the same swatch can't drift apart.

use palantir::Color;

/// Pull `c`'s hue toward `to` by `t`, **keeping `c`'s own alpha**.
///
/// The blend itself is `Color::lerp`; what this adds is the alpha policy, and
/// it's the reason both readers share one function. Opacity on the canvas is
/// owned by a separate rule — the wire set drops to a fixed alpha while a
/// gesture is in flight — so a tier that shifts a *color* must leave it alone,
/// or the two rules multiply and a dimmed wire fades twice.
pub(crate) fn toward(c: Color, to: Color, t: f32) -> Color {
    c.lerp(to, t).with_alpha(c.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toward_blends_the_hue_and_leaves_alpha_alone() {
        let a = Color::linear_rgba(1.0, 0.0, 0.5, 0.8);
        let b = Color::linear_rgba(0.0, 1.0, 0.5, 0.1);
        assert_eq!(toward(a, b, 0.0), a);
        // t = 1 lands on `b`'s rgb but keeps `a`'s alpha — the whole point of
        // this wrapper over the plain `Color::lerp`, which would have taken
        // `b`'s 0.1 along with it.
        let full = toward(a, b, 1.0);
        assert_eq!((full.r, full.g, full.b, full.a), (0.0, 1.0, 0.5, 0.8));
        assert!(
            (a.lerp(b, 1.0).a - b.a).abs() < 1e-6,
            "the bare lerp carries alpha to the far end"
        );
        // Hand-computed midpoint: rgb (0.5, 0.5, 0.5), alpha still 0.8.
        let mid = toward(a, b, 0.5);
        assert_eq!((mid.r, mid.g, mid.b, mid.a), (0.5, 0.5, 0.5, 0.8));
    }
}
