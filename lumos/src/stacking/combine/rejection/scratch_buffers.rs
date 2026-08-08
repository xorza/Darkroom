//! Per-thread working buffers the rejection methods refill for every pixel.

/// Per-thread scratch buffers for stacking combine closures.
///
/// Allocated once per rayon thread via `for_each_init` and reused across all pixels.
#[derive(Debug, Default)]
pub(crate) struct ScratchBuffers {
    /// Tracks original frame indices after rejection reordering.
    pub(crate) indices: Vec<usize>,
    /// Values copied out for a robust centre/spread estimate, leaving the originals untouched.
    pub(crate) estimate_values: Vec<f32>,
    /// Large-N `sort_with_indices`: the value copy it permutes from.
    pub(crate) sort_values: Vec<f32>,
    /// Large-N `sort_with_indices`: the position permutation it sorts.
    pub(crate) sort_permutation: Vec<usize>,
    /// Large-N `sort_with_indices`: the frame-index copy it permutes from.
    pub(crate) sort_indices: Vec<usize>,
    pub(crate) gesd_statistics: Vec<f64>,
    pub(crate) gesd_critical_values: Vec<f64>,
    pub(crate) gesd_sample_count: usize,
    pub(crate) gesd_alpha_bits: u32,
}

impl ScratchBuffers {
    /// Reserve room for `frame_count` samples. The rejection methods clear and refill these per
    /// pixel, so only capacity carries over — a lease reused from the pool is already big enough
    /// and every call after the first is a no-op.
    pub(crate) fn reserve(&mut self, frame_count: usize) {
        self.indices.reserve(frame_count);
        self.estimate_values.reserve(frame_count);
        self.sort_values.reserve(frame_count);
        self.sort_permutation.reserve(frame_count);
        self.sort_indices.reserve(frame_count);
        self.gesd_statistics.reserve(frame_count / 4);
        self.gesd_critical_values.reserve(frame_count / 4);
    }

    /// Restart `indices` as the identity permutation over `n` frames, which is what every
    /// rejection pass starts from before it reorders survivors to the front.
    pub(crate) fn reset_indices(&mut self, n: usize) {
        self.indices.clear();
        self.indices.extend(0..n);
    }

    /// Sort `values[..n]` and `self.indices[..n]` together by value.
    ///
    /// Insertion sort for small N (optimal for typical 10–50 frame stacks) and introsort via
    /// `sort_unstable_by` for large N to avoid O(N^2). The `sort_*` fields exist for the large-N
    /// branch alone; they live here rather than in the function so the allocation survives from
    /// one pixel to the next.
    pub(crate) fn sort_with_indices(&mut self, values: &mut [f32], n: usize) {
        let Self {
            indices,
            sort_values,
            sort_permutation,
            sort_indices,
            ..
        } = self;

        const INSERTION_SORT_THRESHOLD: usize = 64;

        if n <= INSERTION_SORT_THRESHOLD {
            for i in 1..n {
                let mut j = i;
                while j > 0 && values[j - 1] > values[j] {
                    values.swap(j - 1, j);
                    indices.swap(j - 1, j);
                    j -= 1;
                }
            }
        } else {
            // Build position permutation, sort by values, apply to both arrays.
            sort_permutation.clear();
            sort_permutation.extend(0..n);
            sort_permutation.sort_unstable_by(|&a, &b| values[a].total_cmp(&values[b]));

            sort_values.clear();
            sort_values.extend_from_slice(&values[..n]);
            sort_indices.clear();
            sort_indices.extend_from_slice(&indices[..n]);
            for (dst, &src) in sort_permutation.iter().enumerate() {
                values[dst] = sort_values[src];
                indices[dst] = sort_indices[src];
            }
        }
    }
}
