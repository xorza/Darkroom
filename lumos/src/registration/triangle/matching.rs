use glam::DVec2;

use crate::registration::config::TriangleMatchConfig;
use crate::registration::spatial::{KdTree, form_triangles_from_neighbors};

use super::geometry::Triangle;
use super::voting::{PointMatch, build_invariant_tree, resolve_matches, vote_for_correspondences};

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
    resolve_matches(vote_matrix, n_ref, n_target, config.min_votes)
}
