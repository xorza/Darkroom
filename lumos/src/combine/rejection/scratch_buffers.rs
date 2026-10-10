//! Per-thread working buffers the rejection methods refill for every pixel.

use std::ops::Range;

use statrs::distribution::{ContinuousCDF, StudentsT};

use crate::combine::rejection::normal_scores::NormalScores;
use crate::combine::rejection::sorted_samples::SortedSamples;

/// Working state for the GESD test: each removal's statistic, and the critical values by the count
/// of samples a removal is made from.
///
/// A critical value depends only on that count and alpha, not on the pixel, so one table serves
/// every pixel of a run whatever its coverage. Entries are NaN until a pixel needs them.
#[derive(Debug, Default)]
pub(crate) struct GesdScratch {
    pub(crate) statistics: Vec<f64>,
    critical_values: Vec<f64>,
    alpha: f32,
}

impl GesdScratch {
    /// Room for the critical values of up to `frame_count` samples, so no pixel allocates.
    fn reserve(&mut self, frame_count: usize) {
        self.statistics.reserve(frame_count);
        self.critical_values.reserve(frame_count + 1);
    }

    /// Forget the critical values when alpha changed.
    pub(crate) fn prepare(&mut self, alpha: f32) {
        if self.alpha.to_bits() != alpha.to_bits() {
            self.critical_values.clear();
            self.alpha = alpha;
        }
    }

    /// Rosner's critical value for a removal from `live ≥ 3` samples:
    /// `λ = (L − 1)·t / √((L − 2 + t²)·L)`, with `t` the `1 − α/(2L)` quantile of Student's t at
    /// `L − 2` degrees of freedom. At an α no t quantile resolves, t is infinite and λ is its limit
    /// `(L − 1)/√L`.
    pub(crate) fn critical_value(&mut self, live: usize) -> f64 {
        debug_assert!(live >= 3);
        if self.critical_values.len() <= live {
            self.critical_values.resize(live + 1, f64::NAN);
        }
        if self.critical_values[live].is_nan() {
            let count = live as f64;
            let probability = 1.0 - f64::from(self.alpha) / (2.0 * count);
            let t = StudentsT::new(0.0, 1.0, count - 2.0)
                .expect("three samples leave one degree of freedom")
                .inverse_cdf(probability);
            self.critical_values[live] =
                (count - 1.0) / (count * (1.0 + (count - 2.0) / (t * t))).sqrt();
        }
        self.critical_values[live]
    }
}

/// The buffers a rejection method works in while the driver holds the sorted samples.
#[derive(Debug, Default)]
pub(crate) struct MethodScratch {
    /// The winsorized estimate's clamped copy of the window.
    pub(crate) clamped: Vec<f32>,
    pub(crate) scores: NormalScores,
    pub(crate) gesd: GesdScratch,
}

/// Per-thread scratch buffers for the combine.
///
/// Leased from a [`JobScratchPool`](crate::concurrency::job_scratch_pool::JobScratchPool) per job
/// and reused across all of its pixels, so after the first pixel nothing here allocates.
#[derive(Debug, Default)]
pub(crate) struct ScratchBuffers {
    pub(crate) sorted: SortedSamples,
    /// The survivors' weights, in sorted order, for the weighted mean.
    pub(crate) weights: Vec<f32>,
    /// The last pixel's survivors as a window of `sorted`, or `None` when every sample survived
    /// without a sort.
    pub(crate) survivors: Option<Range<usize>>,
    pub(crate) methods: MethodScratch,
}

impl ScratchBuffers {
    /// Reserve room for `frame_count` samples, so the per-pixel refills never allocate.
    pub(crate) fn reserve(&mut self, frame_count: usize) {
        self.sorted.reserve(frame_count);
        self.weights.reserve(frame_count);
        self.methods.clamped.reserve(frame_count);
        self.methods.scores.reserve(frame_count);
        self.methods.gesd.reserve(frame_count);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::combine::rejection::scratch_buffers::ScratchBuffers;

    impl ScratchBuffers {
        /// The gather positions of the last pixel's survivors, or `None` when all of its samples
        /// survived.
        pub(crate) fn survivor_positions(&self) -> Option<&[u32]> {
            self.survivors
                .clone()
                .map(|window| &self.sorted.positions()[window])
        }
    }
}
