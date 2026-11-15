//! [`GaussianTaps`]: one axis of a Gaussian drop's weights, eight taps at a time.

use simd::{F32_LANES, F32x8, Isa, Kernel, Math};

/// The weights of the `2·radius + 1` taps of one axis of a Gaussian drop, into `out`:
/// `exp(−(d² − d₀²)/2σ²)` for `d = step + d₀`, `step` from `−radius` to `radius`, with `d₀`
/// (`nearest`) the offset of the drop's nearest output pixel. The nearest tap is 1 and every other
/// one below it, so the sum stays at least 1 however narrow the drop and however far off a pixel
/// centre it lands, where `exp(−d²/2σ²)` underflows to a sum of 0; the factor `exp(d₀²/2σ²)` it
/// leaves on every tap of the axis cancels in the drop's normalization.
///
/// Every lane, the partial last vector's too, takes [`Math::exp_f32`], within ε, so a tap's weight
/// does not depend on where in the axis it falls.
#[derive(Debug)]
pub(super) struct GaussianTaps<'a> {
    pub(super) radius: isize,
    /// `d₀`, at most half a pixel.
    pub(super) nearest: f64,
    /// `1/2σ²`.
    pub(super) inv_2sigma_sq: f32,
    pub(super) out: &'a mut Vec<f32>,
}

impl Kernel for GaussianTaps<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            radius,
            nearest,
            inv_2sigma_sq,
            out,
        } = self;
        // `d² − d₀² = step·(step + 2d₀)`, exact in f64 for a step of a few pixels.
        out.clear();
        out.extend((-radius..=radius).map(|step| {
            let step = step as f64;
            (step * (step + 2.0 * nearest)) as f32
        }));
        let scale = isa.splat_f32(-inv_2sigma_sq);
        let (chunks, tail) = out.as_chunks_mut::<F32_LANES>();
        for excess in chunks {
            isa.exp_f32(isa.load_f32(excess) * scale).store(excess);
        }
        if !tail.is_empty() {
            isa.exp_f32(isa.load_f32_partial(tail) * scale)
                .store_partial(tail);
        }
    }
}

#[cfg(test)]
mod tests {
    use common::Libm;
    use simd::{IsaTier, Kernel};

    use crate::drizzle::accumulator::gaussian_taps::GaussianTaps;

    /// Every tier against `Portable`, the model `exp_f32` is specified on, at radii that leave no
    /// partial vector, one, and several; and the weights themselves: the nearest tap exactly 1, and
    /// each other one within 2ε of `libm`'s `expf` on the same f32 argument, the route it replaces,
    /// at σ 1, `d₀ = 0.3`: each is within ε of the exact exponential.
    #[test]
    fn the_taps_are_the_shifted_gaussian_on_every_tier() {
        let (nearest, inv_2sigma_sq) = (0.3, 0.5);
        for radius in [1, 3, 4, 8, 11] {
            let taps = |tier: IsaTier| {
                let mut out = Vec::new();
                tier.run(GaussianTaps {
                    radius,
                    nearest,
                    inv_2sigma_sq,
                    out: &mut out,
                });
                out
            };
            let model = taps(IsaTier::portable());
            for tier in IsaTier::supported() {
                assert_eq!(taps(tier), model, "{tier} radius {radius}");
            }
            assert_eq!(model.len(), 2 * radius.unsigned_abs() + 1);
            assert_eq!(model[radius.unsigned_abs()], 1.0);
            for (step, &got) in (-radius..=radius).zip(&model) {
                let step = step as f64;
                let excess = (step * (step + 2.0 * nearest)) as f32;
                let want = (-excess * inv_2sigma_sq).libm_exp();
                assert!(
                    (got - want).abs() <= 2.0 * f32::EPSILON * want,
                    "radius {radius} step {step}: {got} vs {want}"
                );
            }
        }
        let mut out = Vec::new();
        GaussianTaps {
            radius: 2,
            nearest: -0.5,
            inv_2sigma_sq: 8.0,
            out: &mut out,
        }
        .dispatch();
        assert_eq!(out[2], 1.0);
    }
}
