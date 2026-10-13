//! Turning similar triangles into point correspondences.
//!
//! Every pair of matching triangles votes for the three vertex correspondences it implies;
//! the accumulated votes are then resolved greedily so each point is claimed at most once.

/// A reference star paired with a target star, by index into their respective slices.
///
/// The bare pair, shared by everything that carries one: `PointMatch` adds the vote evidence
/// that produced it, and `StarMatch` adds the residual only measurable once a transform exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchIndices {
    /// Index into the reference star slice.
    pub reference: usize,
    /// Index into the target star slice.
    pub target: usize,
}

/// A matched point pair between reference and target, with the confidence its votes earned.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PointMatch {
    pub(crate) indices: MatchIndices,
    /// The pair's votes relative to the most-voted match of the resolved set: 1 for the best.
    pub(crate) confidence: f64,
}

/// Every vote cast, one entry per vote: the pair packed as `reference << 32 | target`.
///
/// Sorted, equal pairs sit together and a run's length is the pair's count: one flat buffer at any
/// star count, visited in `(reference, target)` order. A dense matrix costs `n_ref·n_target` cells
/// whatever the votes, and a hash map visits in an order its seed sets.
#[derive(Debug, Default)]
pub(super) struct VoteMatrix {
    votes: Vec<u64>,
}

impl VoteMatrix {
    #[inline]
    pub(super) fn increment(&mut self, ref_idx: usize, target_idx: usize) {
        debug_assert!(
            u32::try_from(ref_idx).is_ok() && u32::try_from(target_idx).is_ok(),
            "star indices fit in 32 bits"
        );
        self.votes
            .push(((ref_idx as u64) << 32) | target_idx as u64);
    }

    /// Hand every pair that drew votes to `visit`, with how many it drew, in `(reference, target)`
    /// order.
    ///
    /// The pair travels as a [`MatchIndices`] rather than two `usize`s, which are the same type
    /// and so could be handed over transposed without the compiler noticing.
    pub(super) fn for_each_nonzero(&mut self, mut visit: impl FnMut(MatchIndices, usize)) {
        self.votes.sort_unstable();
        for run in self.votes.chunk_by(|a, b| a == b) {
            let pair = MatchIndices {
                reference: (run[0] >> 32) as usize,
                target: (run[0] & u64::from(u32::MAX)) as usize,
            };
            visit(pair, run.len());
        }
    }

    /// The matches the votes resolve to: each pair of at least `min_votes`, most votes first,
    /// greedily, so each of the `n_ref` reference and `n_target` target stars is claimed at most
    /// once.
    pub(super) fn resolve(
        &mut self,
        n_ref: usize,
        n_target: usize,
        min_votes: usize,
    ) -> Vec<PointMatch> {
        let mut voted: Vec<(MatchIndices, usize)> = Vec::new();
        self.for_each_nonzero(|pair, votes| {
            if votes >= min_votes {
                voted.push((pair, votes));
            }
        });

        // By votes, descending; the stable sort keeps the matrix's `(reference, target)` order
        // among ties, so the greedy resolution below takes them the same way every run.
        voted.sort_by(|(_, a_votes), (_, b_votes)| b_votes.cmp(a_votes));

        let mut used_ref = vec![false; n_ref];
        let mut used_target = vec![false; n_target];
        voted.retain(|(pair, _)| {
            let free = !used_ref[pair.reference] && !used_target[pair.target];
            if free {
                used_ref[pair.reference] = true;
                used_target[pair.target] = true;
            }
            free
        });

        // Confidence is relative to the most-voted resolved match — the first, after the sort.
        let max_votes = voted.first().map_or(1, |(_, votes)| *votes);
        voted
            .into_iter()
            .map(|(indices, votes)| PointMatch {
                indices,
                confidence: votes as f64 / max_votes as f64,
            })
            .collect()
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::registration::triangle::voting::VoteMatrix;

    impl VoteMatrix {
        /// Every non-zero entry as an owned `(ref_idx, target_idx, votes)` list — the shape the
        /// tests assert against, which production has no use for.
        pub(crate) fn nonzero_entries(&mut self) -> Vec<(usize, usize, usize)> {
            let mut entries = Vec::new();
            self.for_each_nonzero(|pair, votes| entries.push((pair.reference, pair.target, votes)));
            entries
        }
    }
}
