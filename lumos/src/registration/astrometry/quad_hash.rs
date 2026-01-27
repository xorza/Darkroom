//! Quad-based geometric hashing for star pattern matching.
//!
//! Implements the Astrometry.net / ASTAP approach of using tetrahedron patterns
//! of 4 stars to compute scale and rotation invariant hash codes.

use crate::registration::spatial::KdTree;

/// A quad (tetrahedron) of 4 stars with its geometric hash.
#[derive(Debug, Clone)]
pub struct QuadHash {
    /// Indices of the 4 stars forming this quad
    pub star_indices: [usize; 4],

    /// Hash code: normalized positions of stars C and D in the AB coordinate system
    /// Format: (xC, yC, xD, yD)
    pub code: [f64; 4],
}

impl QuadHash {
    /// Create a new quad hash from star positions.
    ///
    /// The hash code is computed by:
    /// 1. Stars A and B define the coordinate system (A at origin, B at (1,1))
    /// 2. C and D positions are computed in this normalized space
    ///
    /// This makes the code invariant to translation, rotation, and scale.
    pub fn from_positions(indices: [usize; 4], positions: [(f64, f64); 4]) -> Option<Self> {
        let [a, b, c, d] = positions;

        // Vector from A to B
        let ab_x = b.0 - a.0;
        let ab_y = b.1 - a.1;

        // Length of AB (used for normalization)
        let ab_len_sq = ab_x * ab_x + ab_y * ab_y;
        if ab_len_sq < 1e-10 {
            return None; // A and B are too close
        }
        let ab_len = ab_len_sq.sqrt();

        // Create orthonormal coordinate system with A at origin
        // u-axis along AB, v-axis perpendicular
        let u_x = ab_x / ab_len;
        let u_y = ab_y / ab_len;
        let v_x = -u_y;
        let v_y = u_x;

        // Transform C and D into normalized coordinates
        // In this system, A is at (0,0) and B is at (ab_len, 0)
        // We further normalize by ab_len to make B at (1, 0)
        let c_rel_x = c.0 - a.0;
        let c_rel_y = c.1 - a.1;
        let x_c = (c_rel_x * u_x + c_rel_y * u_y) / ab_len;
        let y_c = (c_rel_x * v_x + c_rel_y * v_y) / ab_len;

        let d_rel_x = d.0 - a.0;
        let d_rel_y = d.1 - a.1;
        let x_d = (d_rel_x * u_x + d_rel_y * u_y) / ab_len;
        let y_d = (d_rel_x * v_x + d_rel_y * v_y) / ab_len;

        Some(QuadHash {
            star_indices: indices,
            code: [x_c, y_c, x_d, y_d],
        })
    }

    /// Compute the distance between two hash codes.
    pub fn distance(&self, other: &QuadHash) -> f64 {
        let dx0 = self.code[0] - other.code[0];
        let dy0 = self.code[1] - other.code[1];
        let dx1 = self.code[2] - other.code[2];
        let dy1 = self.code[3] - other.code[3];

        (dx0 * dx0 + dy0 * dy0 + dx1 * dx1 + dy1 * dy1).sqrt()
    }

    /// Check if two quads match within tolerance.
    pub fn matches(&self, other: &QuadHash, tolerance: f64) -> bool {
        self.distance(other) < tolerance
    }
}

/// Builder for creating and matching quad hashes.
#[derive(Debug)]
pub struct QuadHasher {
    /// Maximum number of stars to use for quad formation
    pub max_stars: usize,

    /// Maximum distance between stars in a quad (pixels)
    pub max_quad_radius: f64,

    /// Hash matching tolerance
    pub match_tolerance: f64,
}

impl Default for QuadHasher {
    fn default() -> Self {
        Self {
            max_stars: 100,
            max_quad_radius: 500.0,
            match_tolerance: 0.02,
        }
    }
}

impl QuadHasher {
    /// Create a new quad hasher with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of stars to consider.
    pub fn with_max_stars(mut self, n: usize) -> Self {
        self.max_stars = n;
        self
    }

    /// Set the maximum quad radius in pixels.
    pub fn with_max_quad_radius(mut self, r: f64) -> Self {
        self.max_quad_radius = r;
        self
    }

    /// Set the hash matching tolerance.
    pub fn with_match_tolerance(mut self, t: f64) -> Self {
        self.match_tolerance = t;
        self
    }

    /// Build quad hashes from a list of star positions.
    ///
    /// Uses the N brightest stars (positions should be sorted by brightness).
    /// For each star, forms quads with its 3 nearest neighbors.
    pub fn build_quads(&self, positions: &[(f64, f64)]) -> Vec<QuadHash> {
        let n = positions.len().min(self.max_stars);
        if n < 4 {
            return Vec::new();
        }

        let positions = &positions[..n];

        // Build k-d tree for efficient nearest neighbor queries
        let tree = match KdTree::build(positions) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut quads = Vec::new();

        // For each star, form quads with its nearest neighbors
        for (i, &pos_i) in positions.iter().enumerate() {
            // Find 4 nearest neighbors (including self)
            let neighbors = tree.k_nearest(pos_i, 5);

            // Filter to get 3 other stars within max radius
            let mut other_indices: Vec<usize> = neighbors
                .iter()
                .filter_map(|&(idx, dist_sq)| {
                    if idx == i {
                        return None;
                    }
                    let dist = dist_sq.sqrt();
                    if dist <= self.max_quad_radius {
                        Some(idx)
                    } else {
                        None
                    }
                })
                .collect();

            if other_indices.len() < 3 {
                continue;
            }

            // Take the 3 closest
            other_indices.truncate(3);

            // Form quad with consistent ordering
            let mut indices = [i, other_indices[0], other_indices[1], other_indices[2]];
            indices.sort_unstable();

            let quad_positions = [
                positions[indices[0]],
                positions[indices[1]],
                positions[indices[2]],
                positions[indices[3]],
            ];

            // Try all permutations to find one that produces a valid hash
            // We use the ordering that places A and B as the two most distant stars
            if let Some(quad) = self.best_quad_orientation(&indices, &quad_positions) {
                quads.push(quad);
            }
        }

        quads
    }

    /// Find the best orientation for a quad (A,B as most distant pair).
    fn best_quad_orientation(
        &self,
        indices: &[usize; 4],
        positions: &[(f64, f64); 4],
    ) -> Option<QuadHash> {
        // Find the pair with maximum distance
        let mut max_dist_sq = 0.0;
        let mut ab_pair = (0, 1);

        for i in 0..4 {
            for j in (i + 1)..4 {
                let dx = positions[i].0 - positions[j].0;
                let dy = positions[i].1 - positions[j].1;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq > max_dist_sq {
                    max_dist_sq = dist_sq;
                    ab_pair = (i, j);
                }
            }
        }

        // Get the other two indices for C and D
        let mut cd_indices: Vec<usize> = (0..4)
            .filter(|&x| x != ab_pair.0 && x != ab_pair.1)
            .collect();
        cd_indices.sort_unstable();

        // Reorder indices: A, B, C, D
        let reordered_indices = [
            indices[ab_pair.0],
            indices[ab_pair.1],
            indices[cd_indices[0]],
            indices[cd_indices[1]],
        ];

        let reordered_positions = [
            positions[ab_pair.0],
            positions[ab_pair.1],
            positions[cd_indices[0]],
            positions[cd_indices[1]],
        ];

        QuadHash::from_positions(reordered_indices, reordered_positions)
    }

    /// Match quads between two sets and return matching pairs.
    ///
    /// Returns pairs of (image_quad_idx, catalog_quad_idx) that match.
    pub fn match_quads(
        &self,
        image_quads: &[QuadHash],
        catalog_quads: &[QuadHash],
    ) -> Vec<(usize, usize)> {
        // Build a k-d tree in 4D code space for catalog quads
        // For simplicity, we do a linear search here
        // A production implementation would use a proper 4D tree

        let mut matches = Vec::new();

        for (i, img_quad) in image_quads.iter().enumerate() {
            for (j, cat_quad) in catalog_quads.iter().enumerate() {
                if img_quad.matches(cat_quad, self.match_tolerance) {
                    matches.push((i, j));
                }
            }
        }

        matches
    }

    /// Match quads and return star correspondences.
    ///
    /// Returns pairs of (image_star_idx, catalog_star_idx) based on quad matches.
    pub fn find_star_matches(
        &self,
        image_quads: &[QuadHash],
        catalog_quads: &[QuadHash],
    ) -> Vec<(usize, usize)> {
        let quad_matches = self.match_quads(image_quads, catalog_quads);

        // Vote for star correspondences
        use std::collections::HashMap;
        let mut votes: HashMap<(usize, usize), usize> = HashMap::new();

        for (img_idx, cat_idx) in quad_matches {
            let img_stars = &image_quads[img_idx].star_indices;
            let cat_stars = &catalog_quads[cat_idx].star_indices;

            // All 4 stars in the matched quads correspond
            for k in 0..4 {
                *votes.entry((img_stars[k], cat_stars[k])).or_insert(0) += 1;
            }
        }

        // Return matches with votes > 0, sorted by vote count
        let mut matches: Vec<_> = votes.into_iter().collect();
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(pair, _)| pair).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quad_hash_from_positions() {
        // Simple square: A at origin, B at (1,0), C at (1,1), D at (0,1)
        let positions = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let indices = [0, 1, 2, 3];

        let quad = QuadHash::from_positions(indices, positions).unwrap();

        // In the normalized AB coordinate system:
        // A is at (0, 0), B is at (1, 0)
        // C should be at (1, 1), D at (0, 1)
        assert!((quad.code[0] - 1.0).abs() < 1e-10, "xC = {}", quad.code[0]);
        assert!((quad.code[1] - 1.0).abs() < 1e-10, "yC = {}", quad.code[1]);
        assert!((quad.code[2] - 0.0).abs() < 1e-10, "xD = {}", quad.code[2]);
        assert!((quad.code[3] - 1.0).abs() < 1e-10, "yD = {}", quad.code[3]);
    }

    #[test]
    fn test_quad_hash_scale_invariance() {
        // Same square, but scaled by 2
        let pos1 = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let pos2 = [(0.0, 0.0), (200.0, 0.0), (200.0, 200.0), (0.0, 200.0)];
        let indices = [0, 1, 2, 3];

        let quad1 = QuadHash::from_positions(indices, pos1).unwrap();
        let quad2 = QuadHash::from_positions(indices, pos2).unwrap();

        // Codes should be identical
        for i in 0..4 {
            assert!(
                (quad1.code[i] - quad2.code[i]).abs() < 1e-10,
                "code[{}]: {} vs {}",
                i,
                quad1.code[i],
                quad2.code[i]
            );
        }
    }

    #[test]
    fn test_quad_hash_rotation_invariance() {
        // Square rotated by 45 degrees
        let s = (2.0_f64).sqrt() / 2.0 * 100.0;
        let pos1 = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let pos2 = [(0.0, 0.0), (s, s), (0.0, 2.0 * s), (-s, s)];
        let indices = [0, 1, 2, 3];

        let quad1 = QuadHash::from_positions(indices, pos1).unwrap();
        let quad2 = QuadHash::from_positions(indices, pos2).unwrap();

        // Codes should be identical (or very close)
        for i in 0..4 {
            assert!(
                (quad1.code[i] - quad2.code[i]).abs() < 1e-9,
                "code[{}]: {} vs {}",
                i,
                quad1.code[i],
                quad2.code[i]
            );
        }
    }

    #[test]
    fn test_quad_hash_distance() {
        let q1 = QuadHash {
            star_indices: [0, 1, 2, 3],
            code: [0.5, 0.5, 0.5, 0.5],
        };

        let q2 = QuadHash {
            star_indices: [0, 1, 2, 3],
            code: [0.5, 0.5, 0.5, 0.5],
        };

        let q3 = QuadHash {
            star_indices: [0, 1, 2, 3],
            code: [0.6, 0.5, 0.5, 0.5],
        };

        assert!(q1.distance(&q2) < 1e-10);
        assert!((q1.distance(&q3) - 0.1).abs() < 1e-10);
    }

    #[test]
    fn test_quad_hasher_build() {
        // Create a grid of stars
        let mut positions = Vec::new();
        for y in 0..5 {
            for x in 0..5 {
                positions.push((x as f64 * 50.0, y as f64 * 50.0));
            }
        }

        let hasher = QuadHasher::new()
            .with_max_stars(25)
            .with_max_quad_radius(200.0);

        let quads = hasher.build_quads(&positions);

        // Should have generated some quads
        assert!(!quads.is_empty(), "No quads generated");

        // Each quad should have 4 valid star indices
        for quad in &quads {
            for &idx in &quad.star_indices {
                assert!(idx < positions.len());
            }
        }
    }

    #[test]
    fn test_quad_hasher_match() {
        // Create identical star patterns
        let positions1 = vec![
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 100.0),
            (0.0, 100.0),
            (50.0, 50.0),
        ];

        // Same pattern, translated and scaled
        let scale = 1.5;
        let offset = (50.0, 30.0);
        let positions2: Vec<_> = positions1
            .iter()
            .map(|(x, y)| (x * scale + offset.0, y * scale + offset.1))
            .collect();

        let hasher = QuadHasher::new()
            .with_max_stars(5)
            .with_max_quad_radius(300.0)
            .with_match_tolerance(0.05);

        let quads1 = hasher.build_quads(&positions1);
        let quads2 = hasher.build_quads(&positions2);

        // Should find matching quads
        let matches = hasher.match_quads(&quads1, &quads2);
        assert!(
            !matches.is_empty(),
            "No quad matches found between identical patterns"
        );
    }

    #[test]
    fn test_quad_hasher_insufficient_stars() {
        let positions = vec![(0.0, 0.0), (100.0, 0.0), (50.0, 50.0)];

        let hasher = QuadHasher::new();
        let quads = hasher.build_quads(&positions);

        // Need at least 4 stars
        assert!(quads.is_empty());
    }

    #[test]
    fn test_find_star_matches() {
        // Create matching star patterns
        let positions1 = vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];

        let positions2 = positions1.clone();

        let hasher = QuadHasher::new()
            .with_max_stars(4)
            .with_max_quad_radius(200.0);

        let quads1 = hasher.build_quads(&positions1);
        let quads2 = hasher.build_quads(&positions2);

        let matches = hasher.find_star_matches(&quads1, &quads2);

        // Should find 4 star correspondences
        assert!(!matches.is_empty(), "No star matches found");
    }
}
