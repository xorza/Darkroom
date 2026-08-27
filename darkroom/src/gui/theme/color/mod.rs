//! Color math shared across the GUI's *derived* colours. The theme names the
//! resting colors; the places that blend one — a wire endpoint pulled toward
//! the canvas at rest, a port circle pulled toward white on hover —
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
mod tests;
