//! Tests for triangle matching module.

use glam::DVec2;

use crate::stacking::registration::spatial::KdTree;
use crate::stacking::registration::triangle::TriangleConfig;
use crate::stacking::registration::triangle::geometry::{Orientation, Triangle};
use crate::stacking::registration::triangle::matching::{
    form_triangles_from_neighbors, form_triangles_kdtree, match_triangles,
};
use crate::stacking::registration::triangle::voting::{
    VoteMatrix, build_invariant_tree, resolve_matches, vote_for_correspondences,
};

/// Build a dense VoteMatrix from (ref_idx, target_idx, votes) entries.
fn vote_matrix_from_entries(
    n_ref: usize,
    n_target: usize,
    entries: &[(usize, usize, usize)],
) -> VoteMatrix {
    let mut vm = VoteMatrix::new(n_ref, n_target);
    for &(r, t, count) in entries {
        for _ in 0..count {
            vm.increment(r, t);
        }
    }
    vm
}

mod formation;
mod geometry;
mod invariant;
mod matching;
mod voting;
