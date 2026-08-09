//! Result types for [`crate::math::sum`].

/// A Neumaier-compensated running total: the sum proper plus the compensation term carrying the
/// low-order bits that `sum` could not hold. Only `sum + compensation` is the accurate total, so
/// the two have to travel together — a backend that reduced its lanes and dropped the
/// compensation would silently give back the precision the whole Kahan loop exists to keep.
#[derive(Debug, Clone, Copy)]
pub(super) struct KahanSum {
    pub(super) sum: f32,
    pub(super) compensation: f32,
}
