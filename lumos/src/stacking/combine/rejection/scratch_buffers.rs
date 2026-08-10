//! Per-thread working buffers the rejection methods refill for every pixel.

/// Working buffers for [`ScratchBuffers::sort_with_indices`]' large-N path, which sorts a
/// permutation rather than moving values and so needs a copy of each input beside it.
#[derive(Debug, Default)]
pub(crate) struct SortScratch {
    /// The value copy it permutes from.
    pub(crate) values: Vec<f32>,
    /// The position permutation it sorts.
    pub(crate) permutation: Vec<usize>,
    /// The frame-index copy it permutes from.
    pub(crate) indices: Vec<usize>,
}

/// Working state for the GESD test: the per-iteration statistics, the critical values they are
/// compared against, and the shape those critical values were computed for — a pixel with the
/// same sample count and alpha reuses them instead of recomputing the whole ladder.
#[derive(Debug, Default)]
pub(crate) struct GesdScratch {
    pub(crate) statistics: Vec<f64>,
    pub(crate) critical_values: Vec<f64>,
    pub(crate) sample_count: usize,
    pub(crate) alpha_bits: u32,
}

/// Per-thread scratch buffers for stacking combine closures.
///
/// Allocated once per rayon thread via `for_each_init` and reused across all pixels.
///
/// One lease carries every rejection method's working set, because the pool hands out one object
/// and the method is chosen per pixel. Grouping each method's buffers into its own field keeps
/// that visible: only `indices` and `estimate_values` are shared, and a reader can see at a glance
/// which fields a given method touches.
#[derive(Debug, Default)]
pub(crate) struct ScratchBuffers {
    /// Tracks original frame indices after rejection reordering.
    pub(crate) indices: Vec<usize>,
    /// Values copied out for a robust centre/spread estimate, leaving the originals untouched.
    pub(crate) estimate_values: Vec<f32>,
    pub(crate) sort: SortScratch,
    pub(crate) gesd: GesdScratch,
}

impl ScratchBuffers {
    /// Reserve room for `frame_count` samples. The rejection methods clear and refill these per
    /// pixel, so only capacity carries over — a lease reused from the pool is already big enough
    /// and every call after the first is a no-op.
    pub(crate) fn reserve(&mut self, frame_count: usize) {
        self.indices.reserve(frame_count);
        self.estimate_values.reserve(frame_count);
        self.sort.values.reserve(frame_count);
        self.sort.permutation.reserve(frame_count);
        self.sort.indices.reserve(frame_count);
        // A quarter because that is GESD's automatic outlier ceiling before its cap:
        // `GesdConfig::max_outliers_for_size` is `(n / 4).min(2 or 10)`, so one entry per tested
        // candidate never exceeds this. An explicit `max_outliers` above it simply grows these on
        // first use — `reserve` is a hint, and these buffers are refilled per pixel regardless.
        self.gesd.statistics.reserve(frame_count / 4);
        self.gesd.critical_values.reserve(frame_count / 4);
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
    /// `sort_unstable_by` for large N to avoid O(N^2). [`SortScratch`] exists for the large-N
    /// branch alone; it lives on the struct rather than in the function so the allocation survives
    /// from one pixel to the next.
    pub(crate) fn sort_with_indices(&mut self, values: &mut [f32], n: usize) {
        let Self {
            indices,
            sort:
                SortScratch {
                    values: sort_values,
                    permutation: sort_permutation,
                    indices: sort_indices,
                },
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
