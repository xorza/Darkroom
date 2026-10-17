//! [`ReferenceTriangles`]: the reference frame's triangles, and the votes a target frame's
//! triangles cast against them.

use std::f64::consts::SQRT_2;

use crate::registration::kd_tree::KdTree;
use crate::registration::triangle::geometry::Triangle;
use crate::registration::triangle::triangle_config::TriangleConfig;
use crate::registration::triangle::voting::Votes;

/// The vertex permutations of a triangle: role `i` of one takes role `roles[i]` of the other, and
/// an odd permutation reverses the orientation.
const PERMUTATIONS: [([usize; 3], bool); 6] = [
    ([0, 1, 2], false),
    ([1, 2, 0], false),
    ([2, 0, 1], false),
    ([1, 0, 2], true),
    ([0, 2, 1], true),
    ([2, 1, 0], true),
];

/// The reference frame's triangles, indexed by their invariant ratios for the radius search each
/// target triangle starts its vote with.
#[derive(Debug)]
pub(super) struct ReferenceTriangles {
    triangles: Vec<Triangle>,
    /// Each triangle's ratios as a point, at the triangle's own index.
    invariants: KdTree,
}

impl ReferenceTriangles {
    /// `triangles` indexed by their ratios; `None` for no triangles.
    pub(super) fn new(triangles: Vec<Triangle>) -> Option<Self> {
        let invariants = KdTree::build(triangles.iter().map(|t| t.ratios).collect())?;
        Some(Self {
            triangles,
            invariants,
        })
    }

    /// The votes `target_triangles` cast: each pair of similar triangles votes for the vertex
    /// correspondences their sorted side lengths imply. Where two sides are equal within the ratio
    /// tolerance, noise can swap their order, and with it the vertex roles and the orientation the
    /// triangle reads; every permutation that trades only such roles votes too, its orientation
    /// test reversed when it is odd.
    pub(super) fn vote(&self, target_triangles: &[Triangle], config: &TriangleConfig) -> Votes {
        let mut votes = Votes::default();
        let mut candidates: Vec<usize> = Vec::new();
        let tolerance = config.ratio_tolerance;

        // The tree searches an L2 circle and `is_similar` tests an L∞ square: √2 times the
        // tolerance is the circle that circumscribes the square, so no candidate at its corners
        // is missed.
        let l2_radius = tolerance * SQRT_2;

        for target_tri in target_triangles {
            let query = target_tri.ratios;
            self.invariants
                .radius_indices_into(query, l2_radius, &mut candidates);

            for &ref_idx in &candidates {
                let ref_tri = &self.triangles[ref_idx];
                if !ref_tri.is_similar(target_tri, tolerance) {
                    continue;
                }
                for (roles, odd) in PERMUTATIONS {
                    let admissible = (0..3).all(|i| {
                        roles[i] == i
                            || ref_tri.ties(i, roles[i], tolerance)
                            || target_tri.ties(i, roles[i], tolerance)
                    });
                    let same_orientation = ref_tri.orientation == target_tri.orientation;
                    if !admissible || (config.check_orientation && same_orientation == odd) {
                        continue;
                    }
                    for (i, &role) in roles.iter().enumerate() {
                        votes.increment(ref_tri.indices[i], target_tri.indices[role]);
                    }
                }
            }
        }

        votes
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::registration::kd_tree::KdTree;
    use crate::registration::triangle::reference_triangles::ReferenceTriangles;

    impl ReferenceTriangles {
        pub(crate) const fn invariants(&self) -> &KdTree {
            &self.invariants
        }
    }
}
