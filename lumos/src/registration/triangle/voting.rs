use std::collections::HashMap;

use glam::DVec2;

use crate::registration::config::TriangleMatchConfig;
use crate::registration::spatial::KdTree;

use super::geometry::Triangle;

/// Threshold for switching between dense and sparse vote matrix storage.
///
/// When `n_ref * n_target < DENSE_VOTE_THRESHOLD`, use a dense Vec<u16> matrix.
/// Otherwise, use a sparse HashMap for memory efficiency.
///
/// Memory analysis at threshold (250,000 entries):
/// - Dense: 250,000 * 2 bytes (u16) = 500 KB
/// - Sparse: Only stores non-zero votes, but each entry costs ~40 bytes
///   (key: 16 bytes + value: 8 bytes + HashMap overhead)
///
/// Dense is faster for small point counts due to direct indexing (O(1) vs hash lookup).
/// For 500x500 points (250K entries), dense is still preferred. Beyond that, sparse wins.
const DENSE_VOTE_THRESHOLD: usize = 250_000;

/// A matched point pair between reference and target.
#[derive(Debug, Clone, Copy)]
pub struct PointMatch {
    pub ref_idx: usize,
    pub target_idx: usize,
    pub votes: usize,
    pub confidence: f64,
}

/// Vote matrix storage - either dense (Vec) or sparse (HashMap).
pub(crate) enum VoteMatrix {
    /// Dense storage for small point counts: votes[ref_idx * n_target + target_idx]
    Dense { votes: Vec<u16>, n_target: usize },
    /// Sparse storage for large point counts (u32 saves memory vs usize)
    Sparse(HashMap<(usize, usize), u32>),
}

impl VoteMatrix {
    pub fn new(n_ref: usize, n_target: usize) -> Self {
        let size = n_ref * n_target;
        if size < DENSE_VOTE_THRESHOLD {
            VoteMatrix::Dense {
                votes: vec![0u16; size],
                n_target,
            }
        } else {
            VoteMatrix::Sparse(HashMap::new())
        }
    }

    #[inline]
    pub fn increment(&mut self, ref_idx: usize, target_idx: usize) {
        match self {
            VoteMatrix::Dense { votes, n_target } => {
                let idx = ref_idx * *n_target + target_idx;
                let new_val = votes[idx].saturating_add(1);
                debug_assert!(
                    new_val < u16::MAX,
                    "Vote overflow: too many matching triangles for point pair ({}, {})",
                    ref_idx,
                    target_idx
                );
                votes[idx] = new_val;
            }
            VoteMatrix::Sparse(map) => {
                *map.entry((ref_idx, target_idx)).or_insert(0u32) += 1;
            }
        }
    }

    pub fn into_hashmap(self) -> HashMap<(usize, usize), usize> {
        match self {
            VoteMatrix::Dense { votes, n_target } => {
                let mut map = HashMap::new();
                for (idx, &count) in votes.iter().enumerate() {
                    if count > 0 {
                        let ref_idx = idx / n_target;
                        let target_idx = idx % n_target;
                        map.insert((ref_idx, target_idx), count as usize);
                    }
                }
                map
            }
            VoteMatrix::Sparse(map) => map
                .into_iter()
                .map(|(key, count)| (key, count as usize))
                .collect(),
        }
    }
}

/// Build a k-d tree from reference triangle invariant ratios.
///
/// Each triangle's (ratio.0, ratio.1) pair is stored as a 2D point,
/// enabling efficient radius queries in invariant space.
pub(crate) fn build_invariant_tree(triangles: &[Triangle]) -> Option<KdTree> {
    let invariants: Vec<DVec2> = triangles
        .iter()
        .map(|t| DVec2::new(t.ratios.0, t.ratios.1))
        .collect();
    KdTree::build(&invariants)
}

/// Vote for point correspondences based on matching triangles.
///
/// For each pair of similar triangles, votes for vertex correspondences
/// based on the sorted side lengths (vertices correspond by position in sorted order).
///
/// Uses dense matrix for small point counts (faster due to direct indexing),
/// sparse HashMap for large counts (memory efficient).
pub(crate) fn vote_for_correspondences(
    target_triangles: &[Triangle],
    ref_triangles: &[Triangle],
    invariant_tree: &KdTree,
    config: &TriangleMatchConfig,
    n_ref: usize,
    n_target: usize,
) -> HashMap<(usize, usize), usize> {
    let mut vote_matrix = VoteMatrix::new(n_ref, n_target);

    // Pre-allocate candidate buffer to avoid per-triangle allocations
    let mut candidates: Vec<usize> = Vec::new();

    for target_tri in target_triangles {
        let query = DVec2::new(target_tri.ratios.0, target_tri.ratios.1);
        invariant_tree.radius_indices_into(query, config.ratio_tolerance, &mut candidates);

        for &ref_idx in &candidates {
            let ref_tri = &ref_triangles[ref_idx];

            // Check similarity (exact ratio check after spatial pre-filter)
            if !ref_tri.is_similar(target_tri, config.ratio_tolerance) {
                continue;
            }

            // Check orientation if required
            if config.check_orientation && ref_tri.orientation != target_tri.orientation {
                continue;
            }

            // Vote for all three vertex correspondences
            // Since sides are sorted by length, vertices should correspond in order
            for i in 0..3 {
                let ref_pt = ref_tri.indices[i];
                let target_pt = target_tri.indices[i];
                vote_matrix.increment(ref_pt, target_pt);
            }
        }
    }

    vote_matrix.into_hashmap()
}

/// Resolve vote matrix into final matches using greedy conflict resolution.
///
/// Filters matches by minimum votes, sorts by vote count, and greedily assigns
/// matches ensuring each reference and target point is used at most once.
pub(crate) fn resolve_matches(
    vote_matrix: HashMap<(usize, usize), usize>,
    n_ref: usize,
    n_target: usize,
    min_votes: usize,
) -> Vec<PointMatch> {
    // Filter by minimum votes and collect matches
    let mut matches: Vec<PointMatch> = vote_matrix
        .into_iter()
        .filter(|&(_, votes)| votes >= min_votes)
        .map(|((ref_idx, target_idx), votes)| PointMatch {
            ref_idx,
            target_idx,
            votes,
            confidence: 0.0, // Will be computed below
        })
        .collect();

    // Sort by votes (descending)
    matches.sort_by(|a, b| b.votes.cmp(&a.votes));

    // Resolve one-to-many conflicts (greedy approach)
    let mut used_ref = vec![false; n_ref];
    let mut used_target = vec![false; n_target];
    let mut resolved = Vec::new();

    for m in matches {
        if m.ref_idx < n_ref
            && m.target_idx < n_target
            && !used_ref[m.ref_idx]
            && !used_target[m.target_idx]
        {
            used_ref[m.ref_idx] = true;
            used_target[m.target_idx] = true;
            resolved.push(m);
        }
    }

    // Compute confidence relative to maximum vote count in the resolved set.
    // This gives a meaningful relative ranking (1.0 = best match).
    let max_votes = resolved.iter().map(|m| m.votes).max().unwrap_or(1);
    for m in &mut resolved {
        m.confidence = m.votes as f64 / max_votes as f64;
    }

    resolved
}
