//! The Lanczos windowed-sinc kernel, shared by everything that resamples.

use std::f32::consts::PI;

/// Below this the sinc ratio is `0/0`; the kernel's limit there is 1.
const SINC_ZERO_THRESHOLD: f32 = 1e-6;

/// `sinc(πx) · sinc(πx/a)` — the Lanczos kernel of half-width `a`, zero beyond it.
///
/// One definition for both resamplers: `registration::resample` builds its lookup table from this,
/// and `drizzle` evaluates it per drop for the Lanczos kernel. They are peer subsystems, so it lives
/// here rather than in either of them.
#[inline]
pub(crate) fn kernel(x: f32, a: f32) -> f32 {
    if x.abs() < SINC_ZERO_THRESHOLD {
        return 1.0;
    }
    if x.abs() >= a {
        return 0.0;
    }
    let pi_x = PI * x;
    let pi_x_a = pi_x / a;
    (pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)
}
