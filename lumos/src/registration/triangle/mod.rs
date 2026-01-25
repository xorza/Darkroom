//! Triangle matching for star pattern recognition.
//!
//! This module implements geometric hashing based on triangles formed from stars.
//! Triangles are characterized by their side ratios, which are invariant to
//! translation, rotation, and scale.

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

use std::collections::HashMap;

use crate::registration::spatial::{KdTree, form_triangles_from_neighbors};
use crate::registration::types::StarMatch;

/// Orientation of a triangle (clockwise or counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Clockwise,
    CounterClockwise,
}

/// A triangle formed from three stars.
#[derive(Debug, Clone)]
pub struct Triangle {
    /// Indices of the three stars in the original list.
    pub star_indices: [usize; 3],
    /// Side lengths sorted: sides[0] <= sides[1] <= sides[2].
    pub sides: [f64; 3],
    /// Invariant ratios: (sides[0]/sides[2], sides[1]/sides[2]).
    pub ratios: (f64, f64),
    /// Orientation of the triangle.
    pub orientation: Orientation,
}

impl Triangle {
    /// Create a triangle from three star positions.
    ///
    /// Returns None if the triangle is degenerate (collinear points).
    pub fn from_positions(indices: [usize; 3], positions: [(f64, f64); 3]) -> Option<Self> {
        let (x0, y0) = positions[0];
        let (x1, y1) = positions[1];
        let (x2, y2) = positions[2];

        // Compute side lengths
        let d01 = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let d12 = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        let d20 = ((x0 - x2).powi(2) + (y0 - y2).powi(2)).sqrt();

        // Check for degenerate triangle
        let min_side = 1e-10;
        if d01 < min_side || d12 < min_side || d20 < min_side {
            return None;
        }

        // Sort sides and track which vertices are at each position
        let mut side_vertex_pairs = [(d01, 2), (d12, 0), (d20, 1)];
        side_vertex_pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let sides = [
            side_vertex_pairs[0].0,
            side_vertex_pairs[1].0,
            side_vertex_pairs[2].0,
        ];

        // Compute invariant ratios
        let longest = sides[2];
        if longest < min_side {
            return None;
        }

        let ratios = (sides[0] / longest, sides[1] / longest);

        // Check triangle inequality and area (avoid very flat triangles)
        if sides[0] + sides[1] <= sides[2] * 1.001 {
            return None; // Nearly collinear
        }

        // Compute orientation using cross product
        let cross = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
        if cross.abs() < 1e-10 * longest * longest {
            return None; // Degenerate
        }

        let orientation = if cross > 0.0 {
            Orientation::CounterClockwise
        } else {
            Orientation::Clockwise
        };

        Some(Self {
            star_indices: indices,
            sides,
            ratios,
            orientation,
        })
    }

    /// Check if two triangles are similar within tolerance.
    pub fn is_similar(&self, other: &Triangle, tolerance: f64) -> bool {
        let dr0 = (self.ratios.0 - other.ratios.0).abs();
        let dr1 = (self.ratios.1 - other.ratios.1).abs();
        dr0 < tolerance && dr1 < tolerance
    }

    /// Compute hash key for geometric hashing.
    /// Returns (bin_x, bin_y) where each is in [0, bins).
    pub fn hash_key(&self, bins: usize) -> (usize, usize) {
        // Ratios are in (0, 1], map to bins
        let bin_x = ((self.ratios.0 * bins as f64) as usize).min(bins - 1);
        let bin_y = ((self.ratios.1 * bins as f64) as usize).min(bins - 1);
        (bin_x, bin_y)
    }

    /// Get the vertex opposite to the shortest side.
    pub fn vertex_opposite_shortest(&self) -> usize {
        // Shortest side is sides[0], which is opposite to some vertex
        // We need to track this during construction
        // For now, return the first index
        self.star_indices[0]
    }
}

/// Hash table for fast triangle lookup using geometric hashing.
#[derive(Debug)]
pub struct TriangleHashTable {
    /// 2D grid of bins, each containing triangle indices.
    table: Vec<Vec<usize>>,
    /// Number of bins per dimension.
    bins: usize,
}

impl TriangleHashTable {
    /// Build a hash table from a list of triangles.
    pub fn build(triangles: &[Triangle], bins: usize) -> Self {
        let mut table = vec![Vec::new(); bins * bins];

        for (idx, triangle) in triangles.iter().enumerate() {
            let (bx, by) = triangle.hash_key(bins);
            table[by * bins + bx].push(idx);
        }

        Self { table, bins }
    }

    /// Find candidate triangles that might match the query.
    /// Returns indices into the original triangle array.
    pub fn find_candidates(&self, query: &Triangle, tolerance: f64) -> Vec<usize> {
        let (bx, by) = query.hash_key(self.bins);

        // Search in neighboring bins based on tolerance
        let bin_tolerance = ((tolerance * self.bins as f64).ceil() as usize).max(1);

        let mut candidates = Vec::new();

        let x_min = bx.saturating_sub(bin_tolerance);
        let x_max = (bx + bin_tolerance + 1).min(self.bins);
        let y_min = by.saturating_sub(bin_tolerance);
        let y_max = (by + bin_tolerance + 1).min(self.bins);

        for y in y_min..y_max {
            for x in x_min..x_max {
                candidates.extend_from_slice(&self.table[y * self.bins + x]);
            }
        }

        candidates
    }

    /// Get the number of triangles in the table.
    pub fn len(&self) -> usize {
        self.table.iter().map(|v| v.len()).sum()
    }

    /// Check if the table is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Configuration for triangle matching.
#[derive(Debug, Clone)]
pub struct TriangleMatchConfig {
    /// Maximum number of stars to use (brightest N).
    pub max_stars: usize,
    /// Tolerance for side ratio comparison.
    pub ratio_tolerance: f64,
    /// Minimum votes required to accept a match.
    pub min_votes: usize,
    /// Number of hash table bins per dimension.
    pub hash_bins: usize,
    /// Check orientation (set false to handle mirrored images).
    pub check_orientation: bool,
}

impl Default for TriangleMatchConfig {
    fn default() -> Self {
        Self {
            max_stars: 50,
            ratio_tolerance: 0.01,
            min_votes: 3,
            hash_bins: 100,
            check_orientation: true,
        }
    }
}

/// Form all triangles from a list of star positions.
pub fn form_triangles(positions: &[(f64, f64)], max_stars: usize) -> Vec<Triangle> {
    let n = positions.len().min(max_stars);
    if n < 3 {
        return Vec::new();
    }

    let mut triangles = Vec::with_capacity(n * (n - 1) * (n - 2) / 6);

    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                if let Some(tri) =
                    Triangle::from_positions([i, j, k], [positions[i], positions[j], positions[k]])
                {
                    triangles.push(tri);
                }
            }
        }
    }

    triangles
}

/// Match stars between reference and target using triangle matching.
///
/// Returns a list of matched star pairs with confidence scores.
pub fn match_stars_triangles(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
    config: &TriangleMatchConfig,
) -> Vec<StarMatch> {
    let n_ref = ref_positions.len().min(config.max_stars);
    let n_target = target_positions.len().min(config.max_stars);

    if n_ref < 3 || n_target < 3 {
        return Vec::new();
    }

    // Form triangles
    let ref_triangles = form_triangles(ref_positions, config.max_stars);
    let target_triangles = form_triangles(target_positions, config.max_stars);

    if ref_triangles.is_empty() || target_triangles.is_empty() {
        return Vec::new();
    }

    // Build hash table for reference triangles
    let hash_table = TriangleHashTable::build(&ref_triangles, config.hash_bins);

    // Vote for star correspondences
    let mut vote_matrix: HashMap<(usize, usize), usize> = HashMap::new();

    for target_tri in &target_triangles {
        let candidates = hash_table.find_candidates(target_tri, config.ratio_tolerance);

        for &ref_idx in &candidates {
            let ref_tri = &ref_triangles[ref_idx];

            // Check similarity
            if !ref_tri.is_similar(target_tri, config.ratio_tolerance) {
                continue;
            }

            // Check orientation if required
            if config.check_orientation && ref_tri.orientation != target_tri.orientation {
                continue;
            }

            // Vote for all three vertex correspondences
            // The correspondence depends on which sides match
            // Since sides are sorted by length, vertices should correspond in order
            for i in 0..3 {
                let ref_star = ref_tri.star_indices[i];
                let target_star = target_tri.star_indices[i];
                *vote_matrix.entry((ref_star, target_star)).or_insert(0) += 1;
            }
        }
    }

    // Filter by minimum votes and resolve conflicts
    let mut matches: Vec<StarMatch> = vote_matrix
        .into_iter()
        .filter(|&(_, votes)| votes >= config.min_votes)
        .map(|((ref_idx, target_idx), votes)| StarMatch {
            ref_idx,
            target_idx,
            votes,
            confidence: 0.0, // Will be computed later
        })
        .collect();

    // Sort by votes (descending)
    matches.sort_by(|a, b| b.votes.cmp(&a.votes));

    // Resolve one-to-many conflicts (greedy approach)
    let mut used_ref = vec![false; n_ref];
    let mut used_target = vec![false; n_target];
    let mut resolved = Vec::new();

    for m in matches {
        if m.ref_idx < n_ref
            && m.target_idx < n_target
            && !used_ref[m.ref_idx]
            && !used_target[m.target_idx]
        {
            used_ref[m.ref_idx] = true;
            used_target[m.target_idx] = true;

            // Compute confidence based on votes
            let max_possible_votes = (n_ref.min(n_target) - 2) * (n_ref.min(n_target) - 1) / 2;
            let confidence = (m.votes as f64 / max_possible_votes.max(1) as f64).min(1.0);

            resolved.push(StarMatch { confidence, ..m });
        }
    }

    resolved
}

/// Convert star matches to point pairs for transformation estimation.
#[allow(clippy::type_complexity)]
pub fn matches_to_point_pairs(
    matches: &[StarMatch],
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let mut ref_points = Vec::with_capacity(matches.len());
    let mut target_points = Vec::with_capacity(matches.len());

    for m in matches {
        if m.ref_idx < ref_positions.len() && m.target_idx < target_positions.len() {
            ref_points.push(ref_positions[m.ref_idx]);
            target_points.push(target_positions[m.target_idx]);
        }
    }

    (ref_points, target_points)
}

/// Form triangles using k-d tree for efficient neighbor lookup.
///
/// This is much more efficient than `form_triangles` for large star counts,
/// reducing complexity from O(n³) to approximately O(n * k²) where k is
/// the number of nearest neighbors considered.
///
/// # Arguments
/// * `positions` - Star positions (x, y)
/// * `k_neighbors` - Number of nearest neighbors to consider for each star
///
/// # Returns
/// Vector of triangles formed from neighboring stars
pub fn form_triangles_kdtree(positions: &[(f64, f64)], k_neighbors: usize) -> Vec<Triangle> {
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

/// Match stars using k-d tree optimized triangle formation.
///
/// This version uses a k-d tree for efficient triangle formation, making it
/// suitable for large star counts (>100 stars). For small star counts (<50),
/// the regular `match_stars_triangles` may be faster due to lower overhead.
///
/// # Arguments
/// * `ref_positions` - Reference image star positions
/// * `target_positions` - Target image star positions
/// * `config` - Triangle matching configuration
///
/// # Returns
/// Vector of matched star pairs with confidence scores
pub fn match_stars_triangles_kdtree(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
    config: &TriangleMatchConfig,
) -> Vec<StarMatch> {
    let n_ref = ref_positions.len().min(config.max_stars);
    let n_target = target_positions.len().min(config.max_stars);

    if n_ref < 3 || n_target < 3 {
        return Vec::new();
    }

    // Limit positions to max_stars
    let ref_pos: Vec<_> = ref_positions
        .iter()
        .take(config.max_stars)
        .copied()
        .collect();
    let target_pos: Vec<_> = target_positions
        .iter()
        .take(config.max_stars)
        .copied()
        .collect();

    // Use k-d tree for triangle formation
    // k_neighbors scales with star count but is capped for efficiency
    let k_neighbors = (n_ref.min(n_target) / 3).clamp(5, 20);

    let ref_triangles = form_triangles_kdtree(&ref_pos, k_neighbors);
    let target_triangles = form_triangles_kdtree(&target_pos, k_neighbors);

    if ref_triangles.is_empty() || target_triangles.is_empty() {
        return Vec::new();
    }

    // Build hash table for reference triangles
    let hash_table = TriangleHashTable::build(&ref_triangles, config.hash_bins);

    // Vote for star correspondences
    let mut vote_matrix: HashMap<(usize, usize), usize> = HashMap::new();

    for target_tri in &target_triangles {
        let candidates = hash_table.find_candidates(target_tri, config.ratio_tolerance);

        for &ref_idx in &candidates {
            let ref_tri = &ref_triangles[ref_idx];

            // Check similarity
            if !ref_tri.is_similar(target_tri, config.ratio_tolerance) {
                continue;
            }

            // Check orientation if required
            if config.check_orientation && ref_tri.orientation != target_tri.orientation {
                continue;
            }

            // Vote for all three vertex correspondences
            for i in 0..3 {
                let ref_star = ref_tri.star_indices[i];
                let target_star = target_tri.star_indices[i];
                *vote_matrix.entry((ref_star, target_star)).or_insert(0) += 1;
            }
        }
    }

    // Filter by minimum votes and resolve conflicts
    let mut matches: Vec<StarMatch> = vote_matrix
        .into_iter()
        .filter(|&(_, votes)| votes >= config.min_votes)
        .map(|((ref_idx, target_idx), votes)| StarMatch {
            ref_idx,
            target_idx,
            votes,
            confidence: 0.0,
        })
        .collect();

    // Sort by votes (descending)
    matches.sort_by(|a, b| b.votes.cmp(&a.votes));

    // Resolve one-to-many conflicts (greedy approach)
    let mut used_ref = vec![false; n_ref];
    let mut used_target = vec![false; n_target];
    let mut resolved = Vec::new();

    for m in matches {
        if m.ref_idx < n_ref
            && m.target_idx < n_target
            && !used_ref[m.ref_idx]
            && !used_target[m.target_idx]
        {
            used_ref[m.ref_idx] = true;
            used_target[m.target_idx] = true;

            // Compute confidence based on votes
            let max_possible_votes = (n_ref.min(n_target) - 2) * (n_ref.min(n_target) - 1) / 2;
            let confidence = (m.votes as f64 / max_possible_votes.max(1) as f64).min(1.0);

            resolved.push(StarMatch { confidence, ..m });
        }
    }

    resolved
}
