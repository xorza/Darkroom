//! [`TriangleConfig`]: how triangle matching pairs the stars of two frames.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::registration::kd_tree::KdTree;
use crate::registration::triangle::matching;
use crate::registration::triangle::reference_triangles::ReferenceTriangles;
use crate::registration::triangle::voting::{PointMatch, Votes};

/// Configuration for triangle matching.
///
/// A triangle matches only one of its own orientation, never its mirror image, as Astroalign
/// matches: the estimators fit no reflection and RANSAC refuses a mirrored hypothesis, so a
/// mirrored match could only vote for wrong pairs. A frame set of mixed row order, the usual
/// source of a mirror, is refused at admission.
#[derive(Debug, Clone)]
pub struct TriangleConfig {
    /// How far two triangles' side ratios may differ and still match. Default: 0.01.
    pub ratio_tolerance: f64,
    /// The votes a star pair needs to be a match. Default: 3.
    pub min_votes: usize,
}

impl TriangleConfig {
    /// The points of `reference` and `target` matched by triangle pattern, with each match's
    /// confidence. Only triangles at least `noise_scale` high over their longest side take part.
    ///
    /// Takes the trees rather than the points so the caller's target tree, which match recovery
    /// queries again afterwards, is built once.
    pub(crate) fn match_trees(
        &self,
        reference: &KdTree,
        target: &KdTree,
        noise_scale: f64,
    ) -> Vec<PointMatch> {
        let ref_count = reference.len();
        let target_count = target.len();
        if ref_count < 3 || target_count < 3 {
            return Vec::new();
        }

        // k = 10 gives C(10, 2) = 45 triangles per star, enough for robust matching (Astroalign
        // takes k = 5); the count grows as k², for diminishing returns (k = 20 gives 190).
        let k_neighbors = (ref_count.min(target_count) / 3).clamp(5, 10);

        let ref_triangles = matching::form_triangles_kdtree(reference, k_neighbors, noise_scale);
        let target_triangles = matching::form_triangles_kdtree(target, k_neighbors, noise_scale);
        if target_triangles.is_empty() {
            return Vec::new();
        }
        let Some(reference_triangles) = ReferenceTriangles::new(ref_triangles) else {
            return Vec::new();
        };
        reference_triangles
            .vote(&target_triangles, self, Votes::new(ref_count, target_count))
            .resolve(self.min_votes)
    }

    /// Validate the matching invariants this config owns.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::new("ratio_tolerance", self.ratio_tolerance)
            .finite_and("finite and in (0, 1)", |value| value > 0.0 && value < 1.0)?;
        ConfigField::count("min_votes", self.min_votes).must_be("at least 1", self.min_votes >= 1)
    }
}

impl Default for TriangleConfig {
    fn default() -> Self {
        Self {
            ratio_tolerance: 0.01,
            min_votes: 3,
        }
    }
}
