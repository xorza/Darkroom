//! Per-thread working buffers the rejection methods refill for every pixel.

use statrs::distribution::{ContinuousCDF, StudentsT};

use crate::combine::rejection::normal_scores::NormalScores;
use crate::combine::rejection::sorted_samples::SortedSamples;

/// Working state for the GESD test: the critical values by the count of samples a removal is made
/// from.
///
/// A critical value depends only on that count and alpha, not on the pixel, so one table serves
/// every pixel of a run whatever its coverage. Entries are NaN until a pixel needs them.
#[derive(Debug, Default)]
pub(crate) struct GesdScratch {
    critical_values: Vec<f64>,
    alpha: f32,
}

impl GesdScratch {
    /// Room for the critical values of up to `frame_count` samples, so no pixel allocates; the
    /// values already found stay.
    fn reserve(&mut self, frame_count: usize) {
        let table = frame_count + 1;
        self.critical_values
            .reserve_exact(table.saturating_sub(self.critical_values.len()));
    }

    /// The bytes [`Self::reserve`] holds for `frame_count` samples.
    const fn heap_bytes(frame_count: usize) -> usize {
        frame_count
            .saturating_add(1)
            .saturating_mul(size_of::<f64>())
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
    pub(super) fn critical_value(&mut self, live: usize) -> f64 {
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
/// Leased from a [`ScratchPool`](crate::scratch_pool::ScratchPool) per job
/// and reused across all of its pixels, so after the first pixel nothing here allocates.
#[derive(Debug, Default)]
pub(crate) struct ScratchBuffers {
    pub(crate) sorted: SortedSamples,
    /// The survivors' weights, in sorted order, for the weighted mean.
    pub(crate) weights: Vec<f32>,
    pub(crate) methods: MethodScratch,
}

impl ScratchBuffers {
    /// Room for `frame_count` samples, so the per-pixel refills never allocate. The buffers a
    /// pixel refills are emptied first, so a lease that held samples keeps its size, not twice it.
    pub(crate) fn reserve(&mut self, frame_count: usize) {
        let Self {
            sorted,
            weights,
            methods,
        } = self;
        sorted.reserve(frame_count);
        weights.clear();
        weights.reserve_exact(frame_count);
        methods.clamped.clear();
        methods.clamped.reserve_exact(frame_count);
        methods.scores.reserve(frame_count);
        methods.gesd.reserve(frame_count);
    }

    /// The bytes the buffers hold at most for `frame_count` samples.
    pub(crate) const fn heap_bytes(frame_count: usize) -> usize {
        SortedSamples::heap_bytes(frame_count)
            .saturating_add(frame_count.saturating_mul(2 * size_of::<f32>()))
            .saturating_add(NormalScores::heap_bytes(frame_count))
            .saturating_add(GesdScratch::heap_bytes(frame_count))
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::rejection::scratch_buffers::ScratchBuffers;

    /// A lease that held a pixel's eight samples keeps room for eight when it is fitted again, not
    /// sixteen: the buffers a pixel refills are emptied before the reserve, and the GESD table
    /// keeps the critical value it found, for 4 samples, beside room for 8 + 1.
    #[test]
    fn a_refit_scratch_keeps_its_size() {
        let mut scratch = ScratchBuffers::default();
        scratch.reserve(8);
        scratch.weights.extend([1.0; 8]);
        scratch.methods.clamped.extend([1.0; 8]);
        scratch.methods.gesd.prepare(0.05);
        let found = scratch.methods.gesd.critical_value(4);
        scratch.reserve(8);
        assert_eq!(scratch.weights.capacity(), 8);
        assert_eq!(scratch.methods.clamped.capacity(), 8);
        assert_eq!(scratch.methods.gesd.critical_values.capacity(), 9);
        assert_eq!(scratch.methods.gesd.critical_value(4), found);
    }
}
