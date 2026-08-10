//! What one reduced pixel carries out of the combine, and the buffers that get it there.

use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;

/// Everything one combine job needs beyond the pixels themselves: the covering frames' values
/// packed to the front, their effective weights in the same order, and the rejection methods'
/// own working buffers.
///
/// Leased from a [`JobScratchPool`](crate::concurrency::JobScratchPool) rather than built in a `for_each_init` init closure, because
/// the row loop below runs once per chunk per channel — a fresh init would rebuild all nine
/// vectors on every one of those.
#[derive(Debug, Default)]
pub(crate) struct CombineScratch {
    pub(super) values: Vec<f32>,
    pub(super) eff_weights: Vec<f32>,
    pub(super) buffers: ScratchBuffers,
}

impl CombineScratch {
    /// Size the gather buffers for `frame_count` frames. `values` and `eff_weights` are indexed
    /// directly, so they need the length, not just the capacity.
    pub(super) fn resize(&mut self, frame_count: usize) {
        self.values.resize(frame_count, 0.0);
        self.eff_weights.resize(frame_count, 0.0);
        self.buffers.reserve(frame_count);
    }
}

/// One reduced channel sample: the combined value, how many samples reached it, and — when the
/// caller asked for the quality planes — the effective weight of the survivors.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CombinedSample {
    pub(crate) value: f32,
    /// Samples that survived rejection. Always tracked: it is a count the reducer already knows,
    /// and quantization-noise propagation keys on it.
    pub(crate) survivor_count: usize,
    pub(super) weight: f32,
    pub(super) linear_variance: f32,
}

impl CombinedSample {
    /// A reduction whose survivors are all the inputs.
    pub(crate) fn from_all(value: f32, weights: &[f32]) -> Self {
        Self::from_survivors(value, weights, weights.len(), 0..weights.len())
    }

    /// A reduction over `survivor_indices` into `weights`, measuring their effective weight.
    pub(crate) fn from_survivors(
        value: f32,
        weights: &[f32],
        survivor_count: usize,
        survivor_indices: impl IntoIterator<Item = usize>,
    ) -> Self {
        let mut weight = 0.0f32;
        let mut weight_squared = 0.0f32;
        for index in survivor_indices {
            let survivor_weight = weights[index];
            weight += survivor_weight;
            weight_squared += survivor_weight * survivor_weight;
        }
        let linear_variance = if weight > 0.0 {
            weight_squared / (weight * weight)
        } else {
            0.0
        };
        Self {
            value,
            survivor_count,
            weight,
            linear_variance,
        }
    }

    /// A reduction for a combine that asked for no quality planes: the walk over survivor weights
    /// would produce two numbers nothing reads, and it costs one pass over the frames per pixel.
    pub(crate) fn value_only(value: f32, survivor_count: usize) -> Self {
        Self {
            value,
            survivor_count,
            weight: 0.0,
            linear_variance: 0.0,
        }
    }
}
