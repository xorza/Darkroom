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

/// Every vote cast, one entry per vote: the pair packed as `reference << 32 | target`.
///
/// Sorted, equal pairs sit together and a run's length is the pair's count: one flat buffer at any
/// star count, visited in `(reference, target)` order. A dense matrix costs `n_ref·n_target` cells
/// whatever the votes, and a hash map visits in an order its seed sets.
#[derive(Debug, Default)]
pub(super) struct Votes {
    votes: Vec<u64>,
}

impl Votes {
    #[inline]
    pub(super) fn increment(&mut self, ref_idx: usize, target_idx: usize) {
        debug_assert!(
            u32::try_from(ref_idx).is_ok() && u32::try_from(target_idx).is_ok(),
            "star indices fit in 32 bits"
        );
        self.votes
            .push(((ref_idx as u64) << 32) | target_idx as u64);
    }

    /// Put the votes in `(reference, target)` order, so each pair's votes are one run.
    pub(super) fn sort(&mut self) {
        self.votes.sort_unstable();
    }

    /// Every pair that drew votes, with how many it drew, in the order of the votes: by
    /// `(reference, target)` once [`Self::sort`]ed. The pair travels as a [`MatchIdxs`] rather
    /// than two `usize`s, which are the same type and so could be handed over transposed without
    /// the compiler noticing.
    pub(super) fn counts(&self) -> impl Iterator<Item = PairVotes> + '_ {
        debug_assert!(self.votes.is_sorted(), "votes are counted once sorted");
        self.votes.chunk_by(|a, b| a == b).map(|run| PairVotes {
            pair: MatchIdxs {
                reference: (run[0] >> 32) as usize,
                target: (run[0] & u64::from(u32::MAX)) as usize,
            },
            votes: run.len(),
        })
    }

    /// The matches the votes resolve to: each pair of at least `min_votes`, most votes first,
    /// greedily, so each of the `n_ref` reference and `n_target` target stars is claimed at most
    /// once.
    pub(super) fn resolve(
        mut self,
        n_ref: usize,
        n_target: usize,
        min_votes: usize,
    ) -> Vec<PointMatch> {
        self.sort();
        let mut voted: Vec<PairVotes> = self
            .counts()
            .filter(|counted| counted.votes >= min_votes)
            .collect();

        // By votes, descending; the stable sort keeps the matrix's `(reference, target)` order
        // among ties, so the greedy resolution below takes them the same way every run.
        voted.sort_by_key(|counted| Reverse(counted.votes));

        let mut used_ref = vec![false; n_ref];
        let mut used_target = vec![false; n_target];
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
        pub(crate) fn nonzero_entries(&mut self) -> Vec<PairVotes> {
            self.sort();
            self.counts().collect()
        }
    }
}
