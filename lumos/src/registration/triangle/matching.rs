//! Triangle formation: the triangles each point forms with its nearest neighbours.

use crate::registration::kd_tree::{KdTree, Neighbor};
use crate::registration::triangle::geometry::Triangle;

/// The triangles over each point's `k_neighbors` nearest neighbours in `tree`, each at least
/// `min_height` high over its longest side: O(n·k²).
pub(super) fn form_triangles_kdtree(
    tree: &KdTree,
    k_neighbors: usize,
    min_height: f64,
) -> Vec<Triangle> {
    let poss = tree.points();
    let triangle_idxs = form_triangles_from_neighbors(tree, k_neighbors);

    triangle_idxs
        .into_iter()
        .filter_map(|[i, j, k]| {
            Triangle::from_poss([i, j, k], [poss[i], poss[j], poss[k]], min_height)
        })
        .collect()
}

/// The vertex indices `[i, j, k]`, `i < j < k`, of every triangle a point forms with two of its `k`
/// nearest neighbours in `tree`: O(n·k²), against the O(n³) of every triple.
pub(super) fn form_triangles_from_neighbors(tree: &KdTree, k: usize) -> Vec<[usize; 3]> {
    let n = tree.len();
    if n < 3 {
        return Vec::new();
    }

    let k = k.min(n - 1);
    let mut triangles = Vec::new();
    // Reused across the loop to avoid a per-star allocation (mirrors `radius_idxs_into`).
    let mut neighbors: Vec<Neighbor> = Vec::new();

    for i in 0..n {
        let point_i = tree.points()[i];
        // The point is its own nearest neighbour, hence `k + 1`.
        tree.k_nearest_into(point_i, k + 1, &mut neighbors);

        for (ni, n1) in neighbors.iter().enumerate() {
            if n1.idx == i {
                continue;
            }
            for n2 in neighbors.iter().skip(ni + 1) {
                if n2.idx == i {
                    continue;
                }

                // Sorted, so the same triangle reached from another vertex dedups.
                let mut tri = [i, n1.idx, n2.idx];
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
