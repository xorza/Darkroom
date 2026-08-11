//! Matched positions in the shape the transform estimators take them.

use glam::DVec2;

/// A set of matched points as two parallel position slices, paired by index.
///
/// The estimators and the distortion fitter take positions, not matches — they know nothing about
/// which catalog entry or which star a point came from — so anything holding matches has to
/// materialize the pair before it can fit. RANSAC does it once per sample and once per
/// re-estimation, thousands of times per frame, which is why this refills in place rather than
/// collecting: the two allocations are made once and reused for the whole run.
#[derive(Debug, Default)]
pub(super) struct PointPairs {
    pub(super) reference: Vec<DVec2>,
    pub(super) target: Vec<DVec2>,
}

impl PointPairs {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            reference: Vec::with_capacity(capacity),
            target: Vec::with_capacity(capacity),
        }
    }

    /// Refill from the points `indices` selects on both sides — a RANSAC sample or inlier set,
    /// where one index names the pair.
    pub(super) fn gather(&mut self, indices: &[usize], reference: &[DVec2], target: &[DVec2]) {
        self.gather_matched(
            indices.iter().map(|&index| (index, index)),
            reference,
            target,
        );
    }

    /// Refill from `(reference, target)` index pairs — the shape a match carries, where the two
    /// sides are indexed independently.
    ///
    /// Both sides are filled in one walk so a pair can never reach one and not the other, which is
    /// the correspondence the estimators rely on and cannot check for themselves.
    pub(super) fn gather_matched(
        &mut self,
        matched: impl ExactSizeIterator<Item = (usize, usize)>,
        reference: &[DVec2],
        target: &[DVec2],
    ) {
        self.reference.clear();
        self.target.clear();
        self.reference.reserve_exact(matched.len());
        self.target.reserve_exact(matched.len());
        for (from_reference, from_target) in matched {
            self.reference.push(reference[from_reference]);
            self.target.push(target[from_target]);
        }
        debug_assert_eq!(self.reference.len(), self.target.len());
    }
}
