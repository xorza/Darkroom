use glam::DVec2;

use crate::registration::config::TriangleMatchConfig;
use crate::registration::spatial::{KdTree, form_triangles_from_neighbors};

use super::geometry::Triangle;
use super::voting::{
    PointMatch, VoteMatrix, build_invariant_tree, resolve_matches, vote_for_correspondences,
};

/// Form triangles using k-d tree for efficient neighbor lookup.
///
/// Uses k-nearest neighbors to form triangles in O(n * k²) complexity.
///
/// # Arguments
/// * `positions` - Point positions
/// * `k_neighbors` - Number of nearest neighbors to consider per point
///
/// # Returns
/// Vector of triangles formed from neighboring points
pub fn form_triangles_kdtree(positions: &[DVec2], k_neighbors: usize) -> Vec<Triangle> {
    let tree = match KdTree::build(positions) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let triangle_indices = form_triangles_from_neighbors(&tree, k_neighbors);

    triangle_indices
        .into_iter()
        .filter_map(|[i, j, k]| {
            Triangle::from_positions([i, j, k], [positions[i], positions[j], positions[k]])
        })
        .collect()
}

/// Match points between reference and target sets using triangle pattern matching.
///
/// Uses a k-d tree for efficient triangle formation, making it suitable for
/// large point counts (>100 points).
///
/// # Arguments
/// * `ref_positions` - Reference point positions
/// * `target_positions` - Target point positions
/// * `config` - Triangle matching configuration
///
/// # Returns
/// Vector of matched point pairs with confidence scores
pub fn match_triangles(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    config: &TriangleMatchConfig,
) -> Vec<PointMatch> {
    let n_ref = ref_positions.len();
    let n_target = target_positions.len();

    if n_ref < 3 || n_target < 3 {
        return Vec::new();
    }

    // k_neighbors scales with point count but is capped for efficiency
    let k_neighbors = (n_ref.min(n_target) / 3).clamp(5, 20);

    let ref_triangles = form_triangles_kdtree(ref_positions, k_neighbors);
    let target_triangles = form_triangles_kdtree(target_positions, k_neighbors);

    if ref_triangles.is_empty() || target_triangles.is_empty() {
        return Vec::new();
    }

    // Build k-d tree on reference triangle invariants for fast lookup
    let invariant_tree = match build_invariant_tree(&ref_triangles) {
        Some(t) => t,
        None => return Vec::new(),
    };

    // Vote for point correspondences and resolve conflicts
    let vote_matrix = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &invariant_tree,
        config,
        n_ref,
        n_target,
    );
    let initial_matches = resolve_matches(vote_matrix, n_ref, n_target, config.min_votes);

    // If two-step matching is disabled or not enough matches, return initial matches
    if !config.two_step_matching || initial_matches.len() < 3 {
        return initial_matches;
    }

    // Phase 2: Transform-guided refinement
    two_step_refine_matches(
        ref_positions,
        target_positions,
        &initial_matches,
        &ref_triangles,
        &target_triangles,
        &invariant_tree,
        config,
    )
}

/// Refine matches using transform-guided two-step strategy.
///
/// Given initial matches, estimates a preliminary transform and uses it to
/// guide a second matching pass with stricter tolerance.
fn two_step_refine_matches(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    initial_matches: &[PointMatch],
    ref_triangles: &[Triangle],
    target_triangles: &[Triangle],
    invariant_tree: &KdTree,
    config: &TriangleMatchConfig,
) -> Vec<PointMatch> {
    // Need at least 3 matches to estimate a transform
    if initial_matches.len() < 3 {
        return initial_matches.to_vec();
    }

    // Estimate preliminary transform from initial matches
    // Using a simple similarity transform (rotation + scale + translation)
    let transform = estimate_similarity_transform(ref_positions, target_positions, initial_matches);

    // If transform estimation failed, return initial matches
    let Some((scale, rotation, translation)) = transform else {
        return initial_matches.to_vec();
    };

    // Transform target positions to reference frame
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();

    let transformed_target: Vec<DVec2> = target_positions
        .iter()
        .map(|p| {
            let x_rot = (p.x * cos_r - p.y * sin_r) * scale + translation.x;
            let y_rot = (p.x * sin_r + p.y * cos_r) * scale + translation.y;
            DVec2::new(x_rot, y_rot)
        })
        .collect();

    // Re-vote with strict tolerance, using transformed positions for guidance
    let strict_tolerance = config.ratio_tolerance * 0.5; // Stricter than original
    let n_ref = ref_positions.len();
    let n_target = target_positions.len();

    let mut vote_matrix = VoteMatrix::new(n_ref, n_target);
    let mut candidates: Vec<usize> = Vec::new();

    // Use position proximity to boost votes
    let position_threshold = compute_position_threshold(ref_positions);

    for target_tri in target_triangles {
        let query = DVec2::new(target_tri.ratios.0, target_tri.ratios.1);
        invariant_tree.radius_indices_into(query, strict_tolerance, &mut candidates);

        for &ref_idx in &candidates {
            let ref_tri = &ref_triangles[ref_idx];

            // Check similarity with strict tolerance
            if !ref_tri.is_similar(target_tri, strict_tolerance) {
                continue;
            }

            if config.check_orientation && ref_tri.orientation != target_tri.orientation {
                continue;
            }

            // Vote with position-weighted bonus
            for i in 0..3 {
                let ref_pt = ref_tri.indices[i];
                let target_pt = target_tri.indices[i];

                // Check if transformed target position is close to reference position
                if ref_pt < ref_positions.len() && target_pt < transformed_target.len() {
                    let ref_pos = ref_positions[ref_pt];
                    let tar_pos = transformed_target[target_pt];
                    let dist_sq = (ref_pos - tar_pos).length_squared();

                    // Only vote if positions are reasonably close
                    if dist_sq < position_threshold * position_threshold {
                        // Extra vote for position consistency
                        vote_matrix.increment(ref_pt, target_pt);
                        vote_matrix.increment(ref_pt, target_pt);
                    } else {
                        // Single vote for triangle match only
                        vote_matrix.increment(ref_pt, target_pt);
                    }
                }
            }
        }
    }

    let refined_matches = resolve_matches(vote_matrix, n_ref, n_target, config.min_votes);

    // Return refined matches if they're better, otherwise keep initial
    if refined_matches.len() >= initial_matches.len() {
        refined_matches
    } else {
        initial_matches.to_vec()
    }
}

/// Estimate a similarity transform (rotation + uniform scale + translation) from matches.
///
/// Returns (scale, rotation_radians, translation) or None if estimation fails.
pub(crate) fn estimate_similarity_transform(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    matches: &[PointMatch],
) -> Option<(f64, f64, DVec2)> {
    if matches.len() < 2 {
        return None;
    }

    // Collect matched point pairs
    let pairs: Vec<(DVec2, DVec2)> = matches
        .iter()
        .filter_map(|m| {
            if m.ref_idx < ref_positions.len() && m.target_idx < target_positions.len() {
                Some((ref_positions[m.ref_idx], target_positions[m.target_idx]))
            } else {
                None
            }
        })
        .collect();

    if pairs.len() < 2 {
        return None;
    }

    // Compute centroids
    let n = pairs.len() as f64;
    let ref_centroid = pairs.iter().fold(DVec2::ZERO, |acc, (r, _)| acc + *r) / n;
    let tar_centroid = pairs.iter().fold(DVec2::ZERO, |acc, (_, t)| acc + *t) / n;

    // Center the points
    let ref_centered: Vec<DVec2> = pairs.iter().map(|(r, _)| *r - ref_centroid).collect();
    let tar_centered: Vec<DVec2> = pairs.iter().map(|(_, t)| *t - tar_centroid).collect();

    // Estimate rotation and scale using least squares
    // Reference: "Closed-form solution of absolute orientation using unit quaternions" (Horn, 1987)
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syx = 0.0;
    let mut syy = 0.0;
    let mut tar_norm_sq = 0.0;

    for i in 0..pairs.len() {
        let r = ref_centered[i];
        let t = tar_centered[i];

        sxx += t.x * r.x;
        sxy += t.x * r.y;
        syx += t.y * r.x;
        syy += t.y * r.y;
        tar_norm_sq += t.length_squared();
    }

    if tar_norm_sq < 1e-10 {
        return None;
    }

    // Rotation angle
    let rotation = (syx - sxy).atan2(sxx + syy);

    // Scale
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();
    let scale_num = sxx * cos_r + syy * cos_r + syx * sin_r - sxy * sin_r;
    let scale = scale_num / tar_norm_sq;

    // Clamp scale to reasonable range
    if !(0.1..=10.0).contains(&scale) {
        return None;
    }

    // Translation
    let translation = DVec2::new(
        ref_centroid.x - scale * (tar_centroid.x * cos_r - tar_centroid.y * sin_r),
        ref_centroid.y - scale * (tar_centroid.x * sin_r + tar_centroid.y * cos_r),
    );

    Some((scale, rotation, translation))
}

/// Compute a reasonable position threshold based on point field density.
pub(crate) fn compute_position_threshold(positions: &[DVec2]) -> f64 {
    if positions.len() < 2 {
        return 100.0; // Default large threshold
    }

    // Find average nearest-neighbor distance
    let mut total_min_dist = 0.0;
    for (i, pi) in positions.iter().enumerate().take(20) {
        let mut min_dist_sq = f64::MAX;
        for (j, pj) in positions.iter().enumerate() {
            if i != j {
                let dist_sq = (*pi - *pj).length_squared();
                min_dist_sq = min_dist_sq.min(dist_sq);
            }
        }
        total_min_dist += min_dist_sq.sqrt();
    }

    let avg_nn_dist = total_min_dist / positions.len().min(20) as f64;

    // Threshold is a multiple of average nearest-neighbor distance
    (avg_nn_dist * 3.0).max(5.0)
}
