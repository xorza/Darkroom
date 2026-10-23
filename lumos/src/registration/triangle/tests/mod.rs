//! Tests for triangle matching module.

use glam::DVec2;

use crate::registration::kd_tree::KdTree;
use crate::registration::match_idxs::MatchIdxs;
use crate::registration::triangle::geometry::{Orientation, Triangle};
use crate::registration::triangle::matching::{
    form_triangles_from_neighbors, form_triangles_kdtree,
};
use crate::registration::triangle::reference_triangles::ReferenceTriangles;
use crate::registration::triangle::triangle_config::TriangleConfig;
use crate::registration::triangle::voting::{PairVotes, PointMatch, Votes};

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

/// Five points none of whose ten triangles is within 0.05 of another on both ratios, and none with
/// two sides within 0.02 of each other (pinned by `irregular_triangles_are_pairwise_dissimilar`),
/// so a triangle can only vote for itself, in its one vertex order.
const IRREGULAR: [DVec2; 5] = [
    DVec2::new(0.0, 0.0),
    DVec2::new(30.0, 0.0),
    DVec2::new(28.0, 42.0),
    DVec2::new(12.0, 39.0),
    DVec2::new(-5.0, 21.0),
];

/// The noise scale the fixtures are matched at: the floor `max_sigma_from_fwhm` holds every
/// registration to.
const NOISE_SCALE: f64 = 0.5;

/// A tree over `points`, which a fixture never leaves empty.
fn tree(points: &[DVec2]) -> KdTree {
    KdTree::build(points.to_vec()).expect("a fixture has points")
}

/// [`TriangleConfig::match_trees`] over two point sets, building their trees; nothing for an empty
/// set, which has no tree to build.
fn match_points(reference: &[DVec2], target: &[DVec2], config: &TriangleConfig) -> Vec<PointMatch> {
    if reference.is_empty() || target.is_empty() {
        return Vec::new();
    }
    config.match_trees(&tree(reference), &tree(target), NOISE_SCALE)
}

/// [`form_triangles_kdtree`] over a point set; no triangles for an empty one.
fn triangles_of(points: &[DVec2], k_neighbors: usize) -> Vec<Triangle> {
    if points.is_empty() {
        return Vec::new();
    }
    form_triangles_kdtree(&tree(points), k_neighbors, NOISE_SCALE)
}

/// The votes `target`'s triangles cast against `reference`'s, under `config`.
fn cast_votes(target: &[Triangle], reference: Vec<Triangle>, config: &TriangleConfig) -> Votes {
    ReferenceTriangles::new(reference)
        .expect("a fixture has triangles")
        .vote(target, config)
}

/// `votes` for the pair of reference star `reference` and target star `target`.
const fn tally(reference: usize, target: usize, votes: usize) -> PairVotes {
    PairVotes {
        pair: MatchIdxs { reference, target },
        votes,
    }
}

/// Build a `Votes` that casts each entry's votes for its pair.
fn votes_from_entries(entries: &[PairVotes]) -> Votes {
    let mut vm = Votes::default();
    for &PairVotes { pair, votes } in entries {
        for _ in 0..votes {
            vm.increment(pair.reference, pair.target);
        }
    }
    vm
}

mod formation;
mod geometry;
mod matching;
mod voting;
