//! Turning similar triangles into point correspondences.
//!
//! Every pair of matching triangles votes for the three vertex correspondences it implies;
//! the accumulated votes are then resolved greedily so each point is claimed at most once.

use std::cmp::Reverse;

use crate::registration::match_idxs::MatchIdxs;

/// A matched point pair between reference and target, with the confidence its votes earned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PointMatch {
    pub(crate) idxs: MatchIdxs,
    /// The pair's votes relative to the most-voted match of the resolved set: 1 for the best.
    pub(crate) confidence: f64,
}

/// A pair and the votes it drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PairVotes {
    pub(super) pair: MatchIdxs,
    pub(super) votes: usize,
}

/// The votes each pair drew, a count per `(reference, target)` cell, row-major.
///
/// Dense, at `ref_count·target_count` cells: every pair of similar triangles casts three votes, and
/// their count grows with both star counts' squares — about 4·10⁶ at 500 stars a side — where the
/// cells are 2.5·10⁵, so a list of every vote, sorted to count it, holds and sorts more than the
/// matrix holds. The scan visits the pairs in `(reference, target)` order, as a hash map, seeded,
/// would not.
#[derive(Debug)]
pub(super) struct Votes {
    counts: Vec<u32>,
    target_count: usize,
}

impl Votes {
    /// No votes yet, for `ref_count` reference and `target_count` target stars.
    pub(super) fn new(ref_count: usize, target_count: usize) -> Self {
        Self {
            counts: vec![0; ref_count * target_count],
            target_count,
        }
    }

    #[inline]
    pub(super) fn increment(&mut self, ref_idx: usize, target_idx: usize) {
        debug_assert!(
            target_idx < self.target_count,
            "a target star of the matrix"
        );
        self.counts[ref_idx * self.target_count + target_idx] += 1;
    }

    /// Every pair that drew votes, with how many it drew, by `(reference, target)`. The pair
    /// travels as a [`MatchIdxs`] rather than two `usize`s, which are the same type and so could
    /// be handed over transposed without the compiler noticing.
    pub(super) fn counts(&self) -> impl Iterator<Item = PairVotes> + '_ {
        self.counts
            .iter()
            .enumerate()
            .filter(|&(_, &votes)| votes > 0)
            .map(|(cell, &votes)| PairVotes {
                pair: MatchIdxs {
                    reference: cell / self.target_count,
                    target: cell % self.target_count,
                },
                votes: votes as usize,
            })
    }

    /// The matches the votes resolve to: each pair of at least `min_votes`, most votes first,
    /// greedily, so each reference and each target star is claimed at most once.
    pub(super) fn resolve(self, min_votes: usize) -> Vec<PointMatch> {
        let target_count = self.target_count;
        let ref_count = self.counts.len().checked_div(target_count).unwrap_or(0);
        let mut voted: Vec<PairVotes> = self
            .counts()
            .filter(|counted| counted.votes >= min_votes)
            .collect();

        // By votes, descending; the stable sort keeps the matrix's `(reference, target)` order
        // among ties, so the greedy resolution below takes them the same way every run.
        voted.sort_by_key(|counted| Reverse(counted.votes));

        let mut used_ref = vec![false; ref_count];
        let mut used_target = vec![false; target_count];
        voted.retain(|PairVotes { pair, .. }| {
            let free = !used_ref[pair.reference] && !used_target[pair.target];
            if free {
                used_ref[pair.reference] = true;
                used_target[pair.target] = true;
            }
            free
        });

        // Confidence is relative to the most-voted resolved match — the first, after the sort.
        let max_votes = voted.first().map_or(1, |counted| counted.votes);
        voted
            .into_iter()
            .map(|PairVotes { pair, votes }| PointMatch {
                idxs: pair,
                confidence: votes as f64 / max_votes as f64,
            })
            .collect()
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::registration::triangle::voting::{PairVotes, Votes};

    impl Votes {
        /// Every pair that drew votes, in `(reference, target)` order, as an owned list — the
        /// shape the tests assert against, which production has no use for.
        pub(crate) fn nonzero_entries(&self) -> Vec<PairVotes> {
            self.counts().collect()
        }
    }
}
