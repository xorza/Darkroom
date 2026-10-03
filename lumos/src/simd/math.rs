//! [`Math`]: the transcendental functions the kernels need, built once from [`Isa`] ops so they
//! compute the same bits on every Isa.

use std::f32::consts::LN_2;
use std::f64::consts::LOG2_E;

use crate::simd::{F32x8, F64x4, Isa, Mask8};

/// Cephes `exp()` (Stephen Moshier, public domain): with `x = n·ln 2 + r`,
/// `exp(r) ≈ 1 + 2r·P(r²) / (Q(r²) − r·P(r²))`, to under 2e-13 relative.
const EXP_P: [f64; 3] = [1.261_771_930_748_105_8e-4, 3.029_944_077_074_419_5e-2, 1.0];
const EXP_Q: [f64; 4] = [
    3.001_985_051_386_644_6e-6,
    2.524_483_403_496_841e-3,
    2.272_655_482_081_550_3e-1,
    2.0,
];

/// ln 2 in two parts, the first exact in few enough bits that `n · LN2_HI` is exact for every `n`
/// the clamp admits.
const EXP_LN2_HI: f64 = 6.931_457_519_531_25e-1;
const EXP_LN2_LO: f64 = 1.428_606_820_309_417_3e-6;

/// The arguments past which `exp` leaves the normal range, where it is clamped.
const EXP_MIN: f64 = -708.0;
const EXP_MAX: f64 = 709.0;

/// Cephes single-precision `logf` (`cephes/logf.c`): about 1 ULP on the reduced mantissa.
const LOG_P: [f32; 9] = [
    7.037_683_6e-2,
    -1.151_461e-1,
    1.167_699_9e-1,
    -1.242_014_1e-1,
    1.424_932_3e-1,
    -1.666_805_8e-1,
    2.000_071_5e-1,
    -2.499_999_4e-1,
    3.333_333e-1,
];
const LOG_SQRT_HALF: f32 = 0.707_106_77;

/// ln 2 in two parts, which reassemble the log from the mantissa's and the exponent's.
const LOG_LN2_LO: f32 = -2.121_944_4e-4;
const LOG_LN2_HI: f32 = 0.693_359_4;

/// Where [`Math::asinh_f32`] switches to `asinh(x) = ln(x) + ln 2`: past 2¹², the dropped
/// `1/(4x²)` is under 1.5e-8, below the f32 resolution of a value that large.
pub(crate) const ASINH_LOG_FROM: f32 = 4096.0;

/// Transcendental functions on any [`Isa`]'s vectors.
pub(crate) trait Math: Isa {
    /// `exp(x)` to under 2e-13 relative, `x` clamped to `[−708, 709]` first (a NaN to −708).
    #[inline(always)]
    fn exp_f64(self, x: Self::F64) -> Self::F64 {
        let x = x.min(self.splat_f64(EXP_MAX)).max(self.splat_f64(EXP_MIN));

        let n = x
            .mul_add(self.splat_f64(LOG2_E), self.splat_f64(0.5))
            .floor();
        let r = x - n * self.splat_f64(EXP_LN2_HI) - n * self.splat_f64(EXP_LN2_LO);
        let r2 = r * r;

        let p = self
            .splat_f64(EXP_P[0])
            .mul_add(r2, self.splat_f64(EXP_P[1]))
            .mul_add(r2, self.splat_f64(EXP_P[2]))
            * r;
        let q = self
            .splat_f64(EXP_Q[0])
            .mul_add(r2, self.splat_f64(EXP_Q[1]))
            .mul_add(r2, self.splat_f64(EXP_Q[2]))
            .mul_add(r2, self.splat_f64(EXP_Q[3]));
        let exp_r = self
            .splat_f64(2.0)
            .mul_add(p / (q - p), self.splat_f64(1.0));

        exp_r * n.pow2i()
    }

    /// `ln(x)` for `x > 0` (Cephes `logf`), about 1 ULP. Other lanes give a value the caller
    /// discards, the same on every Isa.
    #[inline(always)]
    fn ln_f32(self, x: Self::F32) -> Self::F32 {
        let one = self.splat_f32(1.0);
        let split = x.frexp();

        // Bring the mantissa into [−0.293, 0.414]: below √½, use 2m − 1 and drop the exponent by
        // one, else m − 1.
        let below = split.mantissa.lanes_lt(self.splat_f32(LOG_SQRT_HALF));
        let exponent = split.exponent - below.keep(one);
        let m = (split.mantissa - one) + below.keep(split.mantissa);

        let z = m * m;
        let mut y = self.splat_f32(LOG_P[0]);
        for &coefficient in &LOG_P[1..] {
            y = y.mul_add(m, self.splat_f32(coefficient));
        }
        y = y * m * z;

        y = exponent.mul_add(self.splat_f32(LOG_LN2_LO), y);
        y = self.splat_f32(-0.5).mul_add(z, y);
        exponent.mul_add(self.splat_f32(LOG_LN2_HI), m + y)
    }

    /// `asinh(x)` for `x ≥ 0`, to a few ULP relative at every magnitude: `log1p(u)` with
    /// `u = x + x²/(1 + √(1 + x²))` — `√(1 + x²) + x − 1` without its cancellation — and
    /// `log1p(u) = ln(1 + u) · u / ((1 + u) − 1)`, whose ratio cancels the rounding of `1 + u`
    /// (Goldberg 1991); past [`ASINH_LOG_FROM`], `ln x + ln 2`. A negative `x` gives a value that
    /// is not positive, or NaN.
    #[inline(always)]
    fn asinh_f32(self, x: Self::F32) -> Self::F32 {
        let one = self.splat_f32(1.0);
        let s = x * x;
        let u = x + s / (one + (one + s).sqrt());
        let w = one + u;
        let dw = w - one;
        // One `ln` serves both forms: of `1 + u` below the switch, of `x` past it.
        let large = x.lanes_gt(self.splat_f32(ASINH_LOG_FROM));
        let log = self.ln_f32(large.select(x, w));
        let log1p = dw.lanes_eq(self.splat_f32(0.0)).select(u, log * (u / dw));
        large.select(log + self.splat_f32(LN_2), log1p)
    }
}

impl<S: Isa> Math for S {}
