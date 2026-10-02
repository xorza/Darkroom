//! Tests for triangle matching module.

use crate::stacking::registration::spatial::KdTree;
use crate::stacking::registration::triangle::TriangleConfig;
use crate::stacking::registration::triangle::geometry::{Orientation, Triangle};
use crate::stacking::registration::triangle::matching::{
    form_triangles_from_neighbors, form_triangles_kdtree, match_triangles,
};
use crate::stacking::registration::triangle::voting::{
    PointMatch, VoteMatrix, build_invariant_tree, resolve_matches, vote_for_correspondences,
};
use crate::testing::prelude::*;

/// A tree over `points`, which a fixture never leaves empty.
fn tree(points: &[DVec2]) -> KdTree {
    KdTree::build(points.to_vec()).expect("a fixture has points")
}

/// [`match_triangles`] over two point sets, building their trees; nothing for an empty set, which
/// has no tree to build.
fn match_points(reference: &[DVec2], target: &[DVec2], config: &TriangleConfig) -> Vec<PointMatch> {
    if reference.is_empty() || target.is_empty() {
        return Vec::new();
    }
    match_triangles(&tree(reference), &tree(target), config)
}

/// [`form_triangles_kdtree`] over a point set; no triangles for an empty one.
fn triangles_of(points: &[DVec2], k_neighbors: usize) -> Vec<Triangle> {
    if points.is_empty() {
        return Vec::new();
    }
    form_triangles_kdtree(&tree(points), k_neighbors)
}

/// Build a dense `VoteMatrix` from (`ref_idx`, `target_idx`, votes) entries.
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
