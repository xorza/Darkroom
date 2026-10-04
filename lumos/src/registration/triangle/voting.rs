//! Turning similar triangles into point correspondences.
//!
//! Every pair of matching triangles votes for the three vertex correspondences it implies;
//! the accumulated votes are then resolved greedily so each point is claimed at most once.

use std::f64::consts::SQRT_2;

use glam::DVec2;

use crate::registration::spatial::KdTree;

use crate::registration::triangle::TriangleConfig;
use crate::registration::triangle::geometry::Triangle;

/// A reference star paired with a target star, by index into their respective slices.
///
/// The bare pair, shared by everything that carries one: [`PointMatch`] adds the vote evidence
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
}

/// Build a k-d tree from reference triangle invariant ratios.
///
/// Each triangle's (ratio.0, ratio.1) pair is stored as a 2D point,
/// enabling efficient radius queries in invariant space.
pub(super) fn build_invariant_tree(triangles: &[Triangle]) -> Option<KdTree> {
    let invariants: Vec<DVec2> = triangles
        .iter()
        .map(|t| DVec2::new(t.ratios.0, t.ratios.1))
        .collect();
    KdTree::build(invariants)
}

/// The vertex permutations of a triangle: role `i` of one takes role `roles[i]` of the other, and
/// an odd permutation reverses the orientation.
const PERMUTATIONS: [([usize; 3], bool); 6] = [
    ([0, 1, 2], false),
    ([1, 2, 0], false),
    ([2, 0, 1], false),
    ([1, 0, 2], true),
    ([0, 2, 1], true),
    ([2, 1, 0], true),
];

/// Whether sides `i` and `j` of either triangle are equal within `tolerance` of the longest, so
/// noise can trade their vertices' roles.
fn tied(a: &Triangle, b: &Triangle, i: usize, j: usize, tolerance: f64) -> bool {
    let ratio = |triangle: &Triangle, side: usize| match side {
        0 => triangle.ratios.0,
        1 => triangle.ratios.1,
        _ => 1.0,
    };
    (ratio(a, i) - ratio(a, j)).abs() < tolerance || (ratio(b, i) - ratio(b, j)).abs() < tolerance
}

/// Vote for point correspondences based on matching triangles.
///
/// For each pair of similar triangles, votes for vertex correspondences
/// based on the sorted side lengths (vertices correspond by position in sorted order). Where two
/// sides are equal within the ratio tolerance, noise can swap their order, and with it the vertex
/// roles and the orientation the triangle reads; every permutation that trades only such roles
/// votes too, its orientation test reversed when it is odd.
///
pub(super) fn vote_for_correspondences(
    target_triangles: &[Triangle],
    ref_triangles: &[Triangle],
    invariant_tree: &KdTree,
    config: &TriangleConfig,
) -> VoteMatrix {
    let mut vote_matrix = VoteMatrix::default();

    // Pre-allocate candidate buffer to avoid per-triangle allocations
    let mut candidates: Vec<usize> = Vec::new();

    // The k-d tree uses L2 distance but is_similar uses L-infinity (per-axis max).
    // Multiply by sqrt(2) so the L2 circle circumscribes the L-inf square,
    // ensuring no valid candidates are missed at the corners.
    let l2_radius = config.ratio_tolerance * SQRT_2;

    for target_tri in target_triangles {
        let query = DVec2::new(target_tri.ratios.0, target_tri.ratios.1);
        invariant_tree.radius_indices_into(query, l2_radius, &mut candidates);

        for &ref_idx in &candidates {
            let ref_tri = &ref_triangles[ref_idx];

            // L-inf filter: exact per-axis tolerance check
            if !ref_tri.is_similar(target_tri, config.ratio_tolerance) {
                continue;
            }

            let tolerance = config.ratio_tolerance;
            for (roles, odd) in PERMUTATIONS {
                let admissible = (0..3)
                    .all(|i| roles[i] == i || tied(ref_tri, target_tri, i, roles[i], tolerance));
                let same_orientation = ref_tri.orientation == target_tri.orientation;
                if !admissible || (config.check_orientation && same_orientation == odd) {
                    continue;
                }
                for (i, &role) in roles.iter().enumerate() {
                    vote_matrix.increment(ref_tri.indices[i], target_tri.indices[role]);
                }
            }
        }
    }

    vote_matrix
}

/// Resolve vote matrix into final matches using greedy conflict resolution.
///
/// Filters matches by minimum votes, sorts by vote count, and greedily assigns
/// matches ensuring each reference and target point is used at most once.
pub(super) fn resolve_matches(
    vote_matrix: &mut VoteMatrix,
    n_ref: usize,
    n_target: usize,
    min_votes: usize,
) -> Vec<PointMatch> {
    // Filter by minimum votes and collect the voted pairs.
    let mut voted: Vec<(MatchIndices, usize)> = Vec::new();
    vote_matrix.for_each_nonzero(|pair, votes| {
        if votes >= min_votes {
            voted.push((pair, votes));
        }
    });

    // By votes, descending; the stable sort keeps the matrix's `(reference, target)` order among
    // ties, so the greedy resolution below takes them the same way every run.
    voted.sort_by(|(_, a_votes), (_, b_votes)| b_votes.cmp(a_votes));

    // Resolve one-to-many conflicts (greedy approach)
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
