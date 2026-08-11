//! The pair of running totals a weighted mean is built from.

use std::ops::Add;

/// The two f64 running totals a weighted mean is built from: `Σ vᵢwᵢ` and `Σ wᵢ`.
///
/// They travel together because the ratio is only meaningful when both come from the same pass over
/// the same elements — a backend that returned one without the other would invite a caller to pair
/// it with a denominator counted over a different set. Keeping the division out of the backends is
/// also what leaves the near-zero-weight policy stated once, at the dispatch point, instead of
/// once per architecture.
#[derive(Debug, Clone, Copy)]
pub(super) struct WeightedSums {
    pub(super) weighted_values: f64,
    pub(super) weight_total: f64,
}

impl Add for WeightedSums {
    type Output = Self;

    /// Joins a vector loop's totals with its scalar tail's.
    fn add(self, other: Self) -> Self {
        Self {
            weighted_values: self.weighted_values + other.weighted_values,
            weight_total: self.weight_total + other.weight_total,
        }
    }
}
