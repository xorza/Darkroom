//! Star-pattern matching: from two point sets to corresponding pairs.
//!
//! Forms triangles over each set's k-nearest neighbours, indexes the reference triangles by their
//! invariants, and hands similar pairs to the voting stage, which turns shared vertices into point
//! correspondences.

use crate::registration::spatial::{KdTree, Neighbor};

use crate::registration::triangle::TriangleConfig;
use crate::registration::triangle::geometry::Triangle;
use crate::registration::triangle::voting::{
    PointMatch, build_invariant_tree, resolve_matches, vote_for_correspondences,
};

/// The triangles over each point's `k_neighbors` nearest neighbours in `tree`, each at least
/// `min_height` high over its longest side: O(n·k²).
pub(super) fn form_triangles_kdtree(
    tree: &KdTree,
    k_neighbors: usize,
    min_height: f64,
) -> Vec<Triangle> {
    let positions = tree.points();
    let triangle_indices = form_triangles_from_neighbors(tree, k_neighbors);

    triangle_indices
        .into_iter()
        .filter_map(|[i, j, k]| {
            Triangle::from_positions(
                [i, j, k],
                [positions[i], positions[j], positions[k]],
                min_height,
            )
        })
        .collect()
}

/// Match the points of two trees by triangle pattern: the matched pairs, with confidence scores.
/// Only triangles at least `noise_scale` high over their longest side take part.
///
/// Takes the trees rather than the points so the caller's target tree, which match recovery
/// queries again afterwards, is built once.
pub(crate) fn match_triangles(
    ref_tree: &KdTree,
    target_tree: &KdTree,
    config: &TriangleConfig,
    noise_scale: f64,
) -> Vec<PointMatch> {
    let n_ref = ref_tree.len();
    let n_target = target_tree.len();

    if n_ref < 3 || n_target < 3 {
        return Vec::new();
    }

    // k_neighbors scales with point count but is capped for efficiency.
    // k=10 gives C(10,2)=45 triangles/star — sufficient for robust matching
    // (Astroalign uses k=5). Higher k increases triangle count quadratically
    // with diminishing returns (k=20 → C(20,2)=190, 4.2× more triangles).
    let k_neighbors = (n_ref.min(n_target) / 3).clamp(5, 10);

    let ref_triangles = form_triangles_kdtree(ref_tree, k_neighbors, noise_scale);
    let target_triangles = form_triangles_kdtree(target_tree, k_neighbors, noise_scale);

    if ref_triangles.is_empty() || target_triangles.is_empty() {
        return Vec::new();
    }

    // Build k-d tree on reference triangle invariants for fast lookup
    let Some(invariant_tree) = build_invariant_tree(&ref_triangles) else {
        return Vec::new();
    };

    // Vote for point correspondences and resolve conflicts
    let mut vote_matrix =
        vote_for_correspondences(&target_triangles, &ref_triangles, &invariant_tree, config);
    resolve_matches(&mut vote_matrix, n_ref, n_target, config.min_votes)
}

/// Form triangles using k-nearest neighbors from a k-d tree.
///
/// This is much more efficient than the brute-force O(n^3) approach,
/// reducing complexity to approximately O(n * k^2) where k is the
/// number of neighbors considered for each star.
///
/// # Arguments
/// * `tree` - K-d tree of star positions
/// * `k` - Number of nearest neighbors to consider for each star
///
/// # Returns
/// Vector of triangle vertex indices [i, j, k] where i < j < k
pub(super) fn form_triangles_from_neighbors(tree: &KdTree, k: usize) -> Vec<[usize; 3]> {
    let n = tree.len();
    if n < 3 {
        return Vec::new();
    }

    let k = k.min(n - 1);
    let mut triangles = Vec::new();
    // Reused across the loop to avoid a per-star allocation (mirrors `radius_indices_into`).
    let mut neighbors: Vec<Neighbor> = Vec::new();

    for i in 0..n {
        let point_i = tree.get_point(i);
        tree.k_nearest_into(point_i, k + 1, &mut neighbors); // +1 because point itself is included

        // Form triangles from pairs of neighbors
        for (ni, n1) in neighbors.iter().enumerate() {
            if n1.index == i {
                continue;
            }
            for n2 in neighbors.iter().skip(ni + 1) {
                if n2.index == i {
                    continue;
                }

                // Normalize triangle indices to avoid duplicates
                let mut tri = [i, n1.index, n2.index];
                tri.sort_unstable();
                triangles.push(tri);
            }
        }
    }

    // Sort + dedup is faster than HashSet for this pattern (cache-friendly,
    // no hashing overhead, and ~50% of entries are duplicates).
    triangles.sort_unstable();
    triangles.dedup();
    triangles
}
