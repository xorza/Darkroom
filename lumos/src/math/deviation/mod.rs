//! Deviation computation.

pub mod scalar;

/// Compute absolute deviations from median in-place.
///
/// Replaces each value with |value - median|.
#[inline]
pub fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    scalar::abs_deviation_inplace(values, median);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
