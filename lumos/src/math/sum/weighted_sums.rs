//! The pair of running totals a weighted mean is built from.

/// The two f64 running totals a weighted mean is built from: `Σ vᵢwᵢ` and `Σ wᵢ`.
///
/// They travel together because the ratio only means something when both come from the same pass
/// over the same elements. Keeping the division out of the kernels leaves the zero-weight policy
/// stated once, in the caller.
#[derive(Debug, Clone, Copy)]
pub(super) struct WeightedSums {
    pub(super) weighted_values: f64,
    pub(super) weight_total: f64,
}
