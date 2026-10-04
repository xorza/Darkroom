//! The Gaussian as a [`BatchModel`]: each lane evaluates `Gaussian2D`'s own unfused expressions,
//! only its `exp` is [`Math::exp_f64`] rather than libm's.

use crate::simd::Isa;
use crate::simd::math::Math;
use crate::star_detection::centroid::simd::{BatchModel, LaneProfile, Sample};

/// The Gaussian at `[x0, y0, amplitude, a, b, c, background]`.
#[derive(Debug, Clone, Copy)]
pub(super) struct GaussianBatch {
    params: [f64; 7],
}

impl GaussianBatch {
    pub(super) const fn new(params: [f64; 7]) -> Self {
        Self { params }
    }
}

impl BatchModel<7> for GaussianBatch {
    type Profile<S: Isa> = Profile<S>;

    #[inline(always)]
    fn profile<S: Isa>(self, isa: S) -> Profile<S> {
        let [x0, y0, amp, a, b, c, bg] = self.params;
        Profile {
            x0: isa.splat_f64(x0),
            y0: isa.splat_f64(y0),
            amp: isa.splat_f64(amp),
            a: isa.splat_f64(a),
            b: isa.splat_f64(b),
            c: isa.splat_f64(c),
            bg: isa.splat_f64(bg),
            neg_half: isa.splat_f64(-0.5),
            one: isa.splat_f64(1.0),
        }
    }
}

/// [`GaussianBatch`]'s parameters, splat across one Isa's lanes.
#[derive(Debug, Clone, Copy)]
pub(super) struct Profile<S: Isa> {
    x0: S::F64,
    y0: S::F64,
    amp: S::F64,
    a: S::F64,
    b: S::F64,
    c: S::F64,
    bg: S::F64,
    neg_half: S::F64,
    one: S::F64,
}

impl<S: Isa> LaneProfile<S, 7> for Profile<S> {
    /// `∂f/∂x0 = A·E·(a·dx + b·dy)`, `∂f/∂y0 = A·E·(b·dx + c·dy)`, `∂f/∂A = E`,
    /// `∂f/∂a = −½A·E·dx²`, `∂f/∂b = −A·E·dx·dy`, `∂f/∂c = −½A·E·dy²`, `∂f/∂B = 1`,
    /// with `E = exp(−½(a·dx² + 2b·dx·dy + c·dy²))`.
    #[inline(always)]
    fn sample(self, isa: S, x: S::F64, y: S::F64) -> Sample<S::F64, 7> {
        let dx = x - self.x0;
        let dy = y - self.y0;
        let t = self.a * dx + self.b * dy;
        let u = self.b * dx + self.c * dy;
        let exp_val = isa.exp_f64(self.neg_half * (dx * t + dy * u));
        let amp_exp = self.amp * exp_val;
        let half_amp_exp = self.neg_half * amp_exp;
        Sample {
            value: amp_exp + self.bg,
            jacobian: [
                amp_exp * t,
                amp_exp * u,
                exp_val,
                half_amp_exp * dx * dx,
                half_amp_exp * dx * (dy + dy),
                half_amp_exp * dy * dy,
                self.one,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::star_detection::centroid::gaussian_fit::Gaussian2D;
    use crate::star_detection::centroid::gaussian_fit::simd::GaussianBatch;
    use crate::star_detection::centroid::lm_optimizer::LMModel;
    use crate::star_detection::centroid::lm_optimizer::internals::ModelStamp;
    use crate::star_detection::centroid::simd::internals::assert_every_tier_matches_portable;

    #[test]
    fn every_tier_matches_portable_bit_for_bit() {
        let mut model = Gaussian2D::new(15.0, 1e-6);
        model.integrate_at(3);
        let stamp = ModelStamp::of(&model, 4, &[1.6, 1.4, 500.0, 0.25, -0.02, 0.18, 50.0]);
        assert_every_tier_matches_portable(
            GaussianBatch::new([1.7, 1.3, 490.0, 0.23, -0.03, 0.17, 51.0]),
            stamp.data(),
            &model.quadrature,
        );
    }
}
