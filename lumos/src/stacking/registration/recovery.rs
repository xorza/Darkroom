//! Recovering matches the triangle vote missed.
//!
//! RANSAC's inliers are only the pairs a minimal sample happened to agree on. Once a transform
//! exists, every unmatched reference star can be projected through it and claimed by whatever
//! target star sits under the prediction — which finds the faint stars triangle matching passed
//! over. Each pass refits from the enlarged set, so the transform and the match list tighten
//! together until neither moves.

use glam::DVec2;

use crate::stacking::registration::point_pairs::PointPairs;
use crate::stacking::registration::ransac::transforms::estimate_transform;
use crate::stacking::registration::spatial::KdTree;
use crate::stacking::registration::transform::{Transform, TransformType};
use crate::stacking::registration::triangle::voting::MatchIndices;

/// Maximum iterations for iterative match recovery.
/// Convergence is typically reached in 2-3 passes; diminishing returns after that.
const RECOVERY_MAX_ITERATIONS: usize = 5;

#[derive(Debug)]
pub(crate) struct RecoveredMatches {
    pub(crate) transform: Transform,
    pub(crate) matches: Vec<MatchIndices>,
}

pub(crate) fn recover_matches(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    transform: &Transform,
    inlier_matches: &[MatchIndices],
    inlier_threshold: f64,
    transform_type: TransformType,
) -> RecoveredMatches {
    let target_tree = match KdTree::build(target_stars) {
        Some(tree) => tree,
        None => {
            return RecoveredMatches {
                transform: *transform,
                matches: inlier_matches.to_vec(),
            };
        }
    };

    let threshold_sq = inlier_threshold * inlier_threshold;
    let mut current_transform = *transform;
    let mut current_matches = inlier_matches.to_vec();

    // Dense small-integer membership over [0, n) → bitmaps, not HashSets: no hashing,
    // no allocation per pass, and order-independent (deterministic).
    let mut matched_target = vec![false; target_stars.len()];
    let mut matched_ref = vec![false; ref_stars.len()];
    // Refit inputs, rebuilt per pass from the pass's own matches; only the capacity carries over.
    let mut all = PointPairs::default();

    for _ in 0..RECOVERY_MAX_ITERATIONS {
        let prev_count = current_matches.len();

        matched_target.fill(false);
        matched_ref.fill(false);
        for star_match in &current_matches {
            matched_target[star_match.target] = true;
            matched_ref[star_match.reference] = true;
        }

        for (ref_idx, &ref_pos) in ref_stars.iter().enumerate() {
            if matched_ref[ref_idx] {
                continue;
            }

            let predicted = current_transform.apply(ref_pos);

            // Claiming the target in `matched_target` is what stops a second reference star from
            // taking it later in this same pass — no separate "newly matched" bitmap needed.
            if let Some(nn) = target_tree.nearest_one(predicted)
                && nn.dist_sq <= threshold_sq
                && !matched_target[nn.index]
            {
                current_matches.push(MatchIndices {
                    reference: ref_idx,
                    target: nn.index,
                });
                matched_target[nn.index] = true;
            }
        }

        // Re-validate all matches against current transform, removing outliers
        current_matches.retain(|star_match| {
            let predicted = current_transform.apply(ref_stars[star_match.reference]);
            (predicted - target_stars[star_match.target]).length_squared() <= threshold_sq
        });

        // Stop if match count didn't change (converged)
        if current_matches.len() == prev_count {
            break;
        }

        // Refit transform with updated matches
        all.gather_matched(
            current_matches
                .iter()
                .map(|star_match| (star_match.reference, star_match.target)),
            ref_stars,
            target_stars,
        );

        match estimate_transform(&all.reference, &all.target, transform_type) {
            Some(new_transform) => current_transform = new_transform,
            None => break,
        }
    }

    // Ensure we never return fewer matches than we started with
    if current_matches.len() < inlier_matches.len() {
        return RecoveredMatches {
            transform: *transform,
            matches: inlier_matches.to_vec(),
        };
    }

    RecoveredMatches {
        transform: current_transform,
        matches: current_matches,
    }
}
