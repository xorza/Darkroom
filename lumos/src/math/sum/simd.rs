//! The sum kernels: f32 samples widened into f64 lanes, folded once at the end.
//!
//! Each kernel splits a vector's eight samples into two f64 halves, accumulates each half in its
//! own lanes, adds the partial last vector zero-padded, and folds the halves' sum by
//! [`F64x4::reduce_sum`]. [`WeightedSumsKernel`] takes exactly [`SumF32`]'s steps for its
//! numerator, so unit weights reproduce the plain sum bit for bit.

use crate::math::sum::weighted_sums::WeightedSums;
use crate::simd::{F32_LANES, F32x8, F64x4, Isa, Kernel};

/// `Σ values` in f64, unrounded.
#[derive(Debug)]
pub(super) struct SumF32<'a>(pub(super) &'a [f32]);

impl Kernel for SumF32<'_> {
    type Output = f64;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> f64 {
        let (chunks, tail) = self.0.as_chunks::<F32_LANES>();
        let mut sum = HalfSums::zero(isa);
        for chunk in chunks {
            sum.add(isa.load_f32(chunk));
        }
        sum.add(isa.load_f32_partial(tail));
        sum.total()
    }
}

/// Both weighted-mean totals over the same elements: `Σ vᵢwᵢ` by [`SumF32`]'s steps, and `Σ wᵢ`.
///
/// Every product is exact in f64 (24 + 24 bits fit in 53), so `v · 1.0` walks the identical values
/// through the identical additions as the plain sum.
#[derive(Debug)]
pub(super) struct WeightedSumsKernel<'a> {
    pub(super) values: &'a [f32],
    pub(super) weights: &'a [f32],
}

impl Kernel for WeightedSumsKernel<'_> {
    type Output = WeightedSums;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> WeightedSums {
        let (value_chunks, value_tail) = self.values.as_chunks::<F32_LANES>();
        let (weight_chunks, weight_tail) = self.weights.as_chunks::<F32_LANES>();
        let mut weighted = HalfSums::zero(isa);
        let mut weight = HalfSums::zero(isa);
        for (values, weights) in value_chunks.iter().zip(weight_chunks) {
            weighted.add_products(isa.load_f32(values), isa.load_f32(weights));
            weight.add(isa.load_f32(weights));
        }
        let weights = isa.load_f32_partial(weight_tail);
        weighted.add_products(isa.load_f32_partial(value_tail), weights);
        weight.add(weights);

        WeightedSums {
            weighted_values: weighted.total(),
            weight_total: weight.total(),
        }
    }
}

/// An f64 running sum of f32 vectors: lanes 0–3 in `low`, lanes 4–7 in `high`.
#[derive(Debug, Clone, Copy)]
struct HalfSums<V> {
    low: V,
    high: V,
}

impl<V: F64x4> HalfSums<V> {
    #[inline(always)]
    fn zero<S: Isa<F64 = V>>(isa: S) -> Self {
        Self {
            low: isa.splat_f64(0.0),
            high: isa.splat_f64(0.0),
        }
    }

    #[inline(always)]
    fn add<F: F32x8<F64 = V>>(&mut self, samples: F) {
        let halves = samples.widen();
        self.low = self.low + halves.low;
        self.high = self.high + halves.high;
    }

    /// Each product exact in f64, so the multiply needs no fusing.
    #[inline(always)]
    fn add_products<F: F32x8<F64 = V>>(&mut self, values: F, weights: F) {
        let (values, weights) = (values.widen(), weights.widen());
        self.low = self.low + values.low * weights.low;
        self.high = self.high + values.high * weights.high;
    }

    /// `(low + high)` folded: lanes `i` and `i + 4` of every vector meet first.
    #[inline(always)]
    fn total(self) -> f64 {
        (self.low + self.high).reduce_sum()
    }
}
