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

/// The 3-4-5 right triangle: sides 3, 4 and 5, so ratios (0.6, 0.8), all exact.
const THREE_FOUR_FIVE: [DVec2; 3] = [
    DVec2::new(0.0, 0.0),
    DVec2::new(3.0, 0.0),
    DVec2::new(0.0, 4.0),
];

/// A unit square of side 10 with its centre: the symmetric field most matching tests start from.
const SQUARE_AND_CENTRE: [DVec2; 5] = [
    DVec2::new(0.0, 0.0),
    DVec2::new(10.0, 0.0),
    DVec2::new(0.0, 10.0),
    DVec2::new(10.0, 10.0),
    DVec2::new(5.0, 5.0),
];

/// Five points none of whose ten triangles is within 0.05 of another on both ratios (pinned by
/// `irregular_triangles_are_pairwise_dissimilar`), so a triangle can only vote for itself.
const IRREGULAR: [DVec2; 5] = [
    DVec2::new(0.0, 0.0),
    DVec2::new(30.0, 0.0),
    DVec2::new(15.0, 40.0),
    DVec2::new(50.0, 20.0),
    DVec2::new(-10.0, 1.0),
];

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
mod matching;
mod voting;
