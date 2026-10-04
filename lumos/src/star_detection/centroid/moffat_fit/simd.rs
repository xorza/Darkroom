//! The Moffat as a [`BatchModel`]: each lane evaluates `MoffatFixedBeta`'s own expressions,
//! `u^(−β)` by its own [`PowStrategy`]. The integer and half-integer powers multiply in `int_pow`'s
//! order, the general one is `exp(−β·ln u)` from [`Math`], which the scalar model takes on the
//! portable Isa, and every division is a division, so a lane is the scalar model bit for bit.

use crate::simd::math::Math;
use crate::simd::{F64x4, Isa};
use crate::star_detection::centroid::moffat_fit::{MoffatFixedBeta, PowStrategy};
use crate::star_detection::centroid::simd::{BatchModel, LaneProfile, Sample};

/// The Moffat at `[x0, y0, amplitude, alpha, background]`.
#[derive(Debug, Clone, Copy)]
pub(super) struct MoffatBatch<'a> {
    model: &'a MoffatFixedBeta,
    params: [f64; 5],
}

impl<'a> MoffatBatch<'a> {
    pub(super) const fn new(model: &'a MoffatFixedBeta, params: [f64; 5]) -> Self {
        Self { model, params }
    }
}

impl BatchModel<5> for MoffatBatch<'_> {
    type Profile<S: Isa> = Profile<S>;

    #[inline(always)]
    fn profile<S: Isa>(self, isa: S) -> Profile<S> {
        let [x0, y0, amp, alpha, bg] = self.params;
        let alpha2 = alpha * alpha;
        Profile {
            x0: isa.splat_f64(x0),
            y0: isa.splat_f64(y0),
            amp: isa.splat_f64(amp),
            alpha: isa.splat_f64(alpha),
            alpha2: isa.splat_f64(alpha2),
            bg: isa.splat_f64(bg),
            one: isa.splat_f64(1.0),
            common_factor: isa.splat_f64(2.0 * amp * self.model.beta / alpha2),
            strategy: self.model.pow_strategy,
        }
    }
}

/// [`MoffatBatch`]'s parameters, splat across one Isa's lanes, with the per-step constants the
/// scalar model derives from them.
#[derive(Debug, Clone, Copy)]
pub(super) struct Profile<S: Isa> {
    x0: S::F64,
    y0: S::F64,
    amp: S::F64,
    alpha: S::F64,
    alpha2: S::F64,
    bg: S::F64,
    one: S::F64,
    /// `2·A·β/α²`, rounded as the scalar model rounds it.
    common_factor: S::F64,
    strategy: PowStrategy,
}

impl<S: Isa> LaneProfile<S, 5> for Profile<S> {
    #[inline(always)]
    fn sample(self, isa: S, x: S::F64, y: S::F64) -> Sample<S::F64, 5> {
        let dx = x - self.x0;
        let dy = y - self.y0;
        let r2 = dx * dx + dy * dy;
        let u = self.one + r2 / self.alpha2;
        let u_neg_beta = self.pow_neg(isa, u);
        let common = self.common_factor * (u_neg_beta / u);
        Sample {
            value: self.amp * u_neg_beta + self.bg,
            jacobian: [
                common * dx,
                common * dy,
                u_neg_beta,
                common * r2 / self.alpha,
                self.one,
            ],
        }
    }
}

impl<S: Isa> Profile<S> {
    /// `fast_pow_neg`: `u^(−β)` by the model's strategy.
    #[inline(always)]
    fn pow_neg(self, isa: S, u: S::F64) -> S::F64 {
        match self.strategy {
            PowStrategy::HalfInt { int_part } => self.one / (self.int_pow(u, int_part) * u.sqrt()),
            PowStrategy::Int { n } => self.one / self.int_pow(u, n),
            PowStrategy::General { neg_beta } => {
                isa.exp_f64(isa.splat_f64(neg_beta) * isa.ln_f64(u))
            }
        }
    }

    /// `u^n` by squaring, `int_pow`'s multiplications in its order.
    #[inline(always)]
    fn int_pow(self, u: S::F64, n: u32) -> S::F64 {
        let (mut result, mut base, mut exp) = (self.one, u, n);
        while exp > 0 {
            if exp & 1 == 1 {
                result = result * base;
            }
            base = base * base;
            exp >>= 1;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::star_detection::centroid::lm_optimizer::LMModel;
    use crate::star_detection::centroid::lm_optimizer::internals::ModelStamp;
    use crate::star_detection::centroid::moffat_fit::MoffatFixedBeta;
    use crate::star_detection::centroid::moffat_fit::simd::MoffatBatch;
    use crate::star_detection::centroid::simd::internals::assert_every_tier_matches_portable;

    /// Under each `PowStrategy`: 2.5 half-integer, 3 integer, 2.3 general.
    #[test]
    fn every_tier_matches_portable_bit_for_bit() {
        for beta in [2.5, 3.0, 2.3] {
            let mut model = MoffatFixedBeta::new(8.0, beta, 1e-6);
            model.integrate_at(3);
            let stamp = ModelStamp::of(&model, 4, &[1.5, 1.5, 800.0, 2.0, 80.0]);
            assert_every_tier_matches_portable(
                MoffatBatch::new(&model, [1.7, 1.3, 790.0, 2.1, 82.0]),
                stamp.data(),
                &model.quadrature,
            );
        }
    }
}
