//! Triangle matching for star pattern recognition.
//!
//! This module implements geometric hashing based on triangles formed from stars.
//! Triangles are characterized by their side ratios, which are invariant to
//! translation, rotation, and scale.
//!
//! # Algorithm Overview
//!
//! 1. Form triangles from star positions using k-nearest neighbors
//! 2. Compute scale-invariant descriptors (sorted side ratios)
//! 3. Hash reference triangles into bins by their ratios
//! 4. For each target triangle, lookup matching reference triangles
//! 5. Vote for star correspondences based on shared vertices
//! 6. Extract high-confidence matches from vote matrix
//!
//! # Implementation Variants
//!
//! This module contains two implementations:
//!
//! - **`match_triangles()`** (production): Uses k-d tree for efficient
//!   neighbor lookup, O(n·k²) complexity where k is neighbors per star.
//!   **This is the public API and should be used in production.**
//!
//! - **`match_stars_triangles()`** (test/bench only): Brute-force O(n³) implementation.
//!   Kept for benchmark comparisons and algorithm validation. Not exported publicly.
//!
//! The brute-force version is marked with `#[cfg_attr]` to allow dead code in
//! non-test/bench builds. Tests use it to validate algorithm correctness independent
//! of the k-d tree optimization.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::registration::constants::{
    DENSE_VOTE_THRESHOLD, MIN_TRIANGLE_AREA_SQ, MIN_TRIANGLE_SIDE,
};
use crate::registration::spatial::{KdTree, form_triangles_from_neighbors};

/// A matched star pair between reference and target.
#[derive(Debug, Clone, Copy)]
pub struct StarMatch {
    pub ref_idx: usize,
    pub target_idx: usize,
    pub votes: usize,
    pub confidence: f64,
}

/// Vote matrix storage - either dense (Vec) or sparse (HashMap).
enum VoteMatrix {
    /// Dense storage for small star counts: votes[ref_idx * n_target + target_idx]
    Dense { votes: Vec<u16>, n_target: usize },
    /// Sparse storage for large star counts
    Sparse(HashMap<(usize, usize), usize>),
}

impl VoteMatrix {
    fn new(n_ref: usize, n_target: usize) -> Self {
        let size = n_ref * n_target;
        if size < DENSE_VOTE_THRESHOLD {
            VoteMatrix::Dense {
                votes: vec![0u16; size],
                n_target,
            }
        } else {
            VoteMatrix::Sparse(HashMap::new())
        }
    }

    #[inline]
    fn increment(&mut self, ref_idx: usize, target_idx: usize) {
        match self {
            VoteMatrix::Dense { votes, n_target } => {
                let idx = ref_idx * *n_target + target_idx;
                let new_val = votes[idx].saturating_add(1);
                debug_assert!(
                    new_val < u16::MAX,
                    "Vote overflow: too many matching triangles for star pair ({}, {})",
                    ref_idx,
                    target_idx
                );
                votes[idx] = new_val;
            }
            VoteMatrix::Sparse(map) => {
                *map.entry((ref_idx, target_idx)).or_insert(0) += 1;
            }
        }
    }

    fn into_hashmap(self) -> HashMap<(usize, usize), usize> {
        match self {
            VoteMatrix::Dense { votes, n_target } => {
                let mut map = HashMap::new();
                for (idx, &count) in votes.iter().enumerate() {
                    if count > 0 {
                        let ref_idx = idx / n_target;
                        let target_idx = idx % n_target;
                        map.insert((ref_idx, target_idx), count as usize);
                    }
                }
                map
            }
            VoteMatrix::Sparse(map) => map,
        }
    }
}

/// Vote for star correspondences based on matching triangles.
///
/// For each pair of similar triangles, votes for vertex correspondences
/// based on the sorted side lengths (vertices correspond by position in sorted order).
///
/// Uses dense matrix for small star counts (faster due to direct indexing),
/// sparse HashMap for large counts (memory efficient).
fn vote_for_correspondences(
    target_triangles: &[Triangle],
    ref_triangles: &[Triangle],
    hash_table: &TriangleHashTable,
    config: &TriangleMatchConfig,
    n_ref: usize,
    n_target: usize,
) -> HashMap<(usize, usize), usize> {
    let mut vote_matrix = VoteMatrix::new(n_ref, n_target);

    // Pre-allocate candidate buffer to avoid per-triangle allocations
    let mut candidates: Vec<usize> = Vec::new();

    for target_tri in target_triangles {
        hash_table.find_candidates_into(target_tri, config.ratio_tolerance, &mut candidates);

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
            // Since sides are sorted by length, vertices should correspond in order
            for i in 0..3 {
                let ref_star = ref_tri.star_indices[i];
                let target_star = target_tri.star_indices[i];
                vote_matrix.increment(ref_star, target_star);
            }
        }
    }

    vote_matrix.into_hashmap()
}

/// Resolve vote matrix into final star matches using greedy conflict resolution.
///
/// Filters matches by minimum votes, sorts by vote count, and greedily assigns
/// matches ensuring each reference and target star is used at most once.
fn resolve_matches(
    vote_matrix: HashMap<(usize, usize), usize>,
    n_ref: usize,
    n_target: usize,
    min_votes: usize,
) -> Vec<StarMatch> {
    // Filter by minimum votes and collect matches
    let mut matches: Vec<StarMatch> = vote_matrix
        .into_iter()
        .filter(|&(_, votes)| votes >= min_votes)
        .map(|((ref_idx, target_idx), votes)| StarMatch {
            ref_idx,
            target_idx,
            votes,
            confidence: 0.0, // Will be computed below
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

/// Orientation of a triangle (clockwise or counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Orientation {
    Clockwise,
    CounterClockwise,
}

/// A triangle formed from three stars.
#[derive(Debug, Clone)]
pub(crate) struct Triangle {
    /// Indices of the three stars in the original list.
    pub star_indices: [usize; 3],
    /// Side lengths sorted: sides[0] <= sides[1] <= sides[2].
    #[allow(dead_code)]
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

        // Check for degenerate triangle (sides too short)
        if d01 < MIN_TRIANGLE_SIDE || d12 < MIN_TRIANGLE_SIDE || d20 < MIN_TRIANGLE_SIDE {
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
        if longest < MIN_TRIANGLE_SIDE {
            return None;
        }

        let ratios = (sides[0] / longest, sides[1] / longest);

        // Check for very flat triangles using Heron's formula for area
        // area² = s(s-a)(s-b)(s-c) where s = (a+b+c)/2
        let s = (sides[0] + sides[1] + sides[2]) / 2.0;
        let area_sq = s * (s - sides[0]) * (s - sides[1]) * (s - sides[2]);
        if area_sq < MIN_TRIANGLE_AREA_SQ {
            return None; // Too flat / nearly collinear
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
}

/// Hash table for fast triangle lookup using geometric hashing.
#[derive(Debug)]
pub(crate) struct TriangleHashTable {
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
    #[cfg(test)]
    pub fn find_candidates(&self, query: &Triangle, tolerance: f64) -> Vec<usize> {
        let mut candidates = Vec::new();
        self.find_candidates_into(query, tolerance, &mut candidates);
        candidates
    }

    /// Find candidate triangles into a pre-allocated buffer.
    ///
    /// The buffer is cleared before use. This avoids allocations when
    /// called repeatedly in a loop.
    pub fn find_candidates_into(
        &self,
        query: &Triangle,
        tolerance: f64,
        candidates: &mut Vec<usize>,
    ) {
        candidates.clear();

        let (bx, by) = query.hash_key(self.bins);

        // Search in neighboring bins based on tolerance
        let bin_tolerance = ((tolerance * self.bins as f64).ceil() as usize).max(1);

        let x_min = bx.saturating_sub(bin_tolerance);
        let x_max = (bx + bin_tolerance + 1).min(self.bins);
        let y_min = by.saturating_sub(bin_tolerance);
        let y_max = (by + bin_tolerance + 1).min(self.bins);

        for y in y_min..y_max {
            for x in x_min..x_max {
                candidates.extend_from_slice(&self.table[y * self.bins + x]);
            }
        }
    }

    /// Get the number of triangles in the table.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.table.iter().map(|v| v.len()).sum()
    }

    /// Check if the table is empty.
    #[cfg(test)]
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
    /// Enable two-step matching (rough then fine).
    /// Phase 1 uses relaxed tolerance (5x ratio_tolerance) for initial matches.
    /// Phase 2 uses strict tolerance (ratio_tolerance) for refinement.
    pub two_step_matching: bool,
}

impl Default for TriangleMatchConfig {
    fn default() -> Self {
        Self {
            max_stars: 50,
            ratio_tolerance: 0.01,
            min_votes: 3,
            hash_bins: 100,
            check_orientation: true,
            two_step_matching: false, // Disabled by default for backwards compatibility
        }
    }
}

/// Form all triangles from a list of star positions (brute-force O(n³)).
///
/// For large star counts, prefer `form_triangles_kdtree` which is O(n·k²).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn form_triangles(positions: &[(f64, f64)], max_stars: usize) -> Vec<Triangle> {
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

/// Match stars between reference and target using triangle matching (brute-force).
///
/// This is the O(n³) brute-force version. For better performance with large star
/// counts (>50 stars), use `match_triangles` instead.
///
/// Returns a list of matched star pairs with confidence scores.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn match_stars_triangles(
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

    // Vote for star correspondences and resolve conflicts
    let vote_matrix = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &hash_table,
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
    // Use initial matches to estimate a preliminary transform, then re-match with stricter tolerance
    // Note: for match_stars_triangles we use the full positions since form_triangles limits internally
    two_step_refine_matches(
        ref_positions,
        target_positions,
        &initial_matches,
        &ref_triangles,
        &target_triangles,
        &hash_table,
        config,
    )
}

/// Refine matches using transform-guided two-step strategy.
///
/// Given initial matches, estimates a preliminary transform and uses it to
/// guide a second matching pass with stricter tolerance.
fn two_step_refine_matches(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
    initial_matches: &[StarMatch],
    ref_triangles: &[Triangle],
    target_triangles: &[Triangle],
    hash_table: &TriangleHashTable,
    config: &TriangleMatchConfig,
) -> Vec<StarMatch> {
    // Need at least 3 matches to estimate a transform
    if initial_matches.len() < 3 {
        return initial_matches.to_vec();
    }

    // Estimate preliminary transform from initial matches
    // Using a simple similarity transform (rotation + scale + translation)
    let transform = estimate_similarity_transform(ref_positions, target_positions, initial_matches);

    // If transform estimation failed, return initial matches
    let Some((scale, rotation, tx, ty)) = transform else {
        return initial_matches.to_vec();
    };

    // Transform target positions to reference frame
    let cos_r = rotation.cos();
    let sin_r = rotation.sin();

    let transformed_target: Vec<(f64, f64)> = target_positions
        .iter()
        .map(|&(x, y)| {
            let x_rot = (x * cos_r - y * sin_r) * scale + tx;
            let y_rot = (x * sin_r + y * cos_r) * scale + ty;
            (x_rot, y_rot)
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
        hash_table.find_candidates_into(target_tri, strict_tolerance, &mut candidates);

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
                let ref_star = ref_tri.star_indices[i];
                let target_star = target_tri.star_indices[i];

                // Check if transformed target position is close to reference position
                if ref_star < ref_positions.len() && target_star < transformed_target.len() {
                    let (rx, ry) = ref_positions[ref_star];
                    let (tx, ty) = transformed_target[target_star];
                    let dist_sq = (rx - tx).powi(2) + (ry - ty).powi(2);

                    // Only vote if positions are reasonably close
                    if dist_sq < position_threshold * position_threshold {
                        // Extra vote for position consistency
                        vote_matrix.increment(ref_star, target_star);
                        vote_matrix.increment(ref_star, target_star);
                    } else {
                        // Single vote for triangle match only
                        vote_matrix.increment(ref_star, target_star);
                    }
                }
            }
        }
    }

    let refined_matches = resolve_matches(
        vote_matrix.into_hashmap(),
        n_ref,
        n_target,
        config.min_votes,
    );

    // Return refined matches if they're better, otherwise keep initial
    if refined_matches.len() >= initial_matches.len() {
        refined_matches
    } else {
        initial_matches.to_vec()
    }
}

/// Estimate a similarity transform (rotation + uniform scale + translation) from matches.
///
/// Returns (scale, rotation_radians, translate_x, translate_y) or None if estimation fails.
fn estimate_similarity_transform(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
    matches: &[StarMatch],
) -> Option<(f64, f64, f64, f64)> {
    if matches.len() < 2 {
        return None;
    }

    // Collect matched point pairs
    let pairs: Vec<((f64, f64), (f64, f64))> = matches
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
    let (ref_cx, ref_cy) = pairs.iter().fold((0.0, 0.0), |(sx, sy), ((rx, ry), _)| {
        (sx + rx / n, sy + ry / n)
    });
    let (tar_cx, tar_cy) = pairs.iter().fold((0.0, 0.0), |(sx, sy), (_, (tx, ty))| {
        (sx + tx / n, sy + ty / n)
    });

    // Center the points
    let ref_centered: Vec<(f64, f64)> = pairs
        .iter()
        .map(|((rx, ry), _)| (rx - ref_cx, ry - ref_cy))
        .collect();
    let tar_centered: Vec<(f64, f64)> = pairs
        .iter()
        .map(|(_, (tx, ty))| (tx - tar_cx, ty - tar_cy))
        .collect();

    // Estimate rotation and scale using least squares
    // Reference: "Closed-form solution of absolute orientation using unit quaternions" (Horn, 1987)
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syx = 0.0;
    let mut syy = 0.0;
    let mut tar_norm_sq = 0.0;

    for i in 0..pairs.len() {
        let (rx, ry) = ref_centered[i];
        let (tx, ty) = tar_centered[i];

        sxx += tx * rx;
        sxy += tx * ry;
        syx += ty * rx;
        syy += ty * ry;
        tar_norm_sq += tx * tx + ty * ty;
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
    let tx = ref_cx - scale * (tar_cx * cos_r - tar_cy * sin_r);
    let ty = ref_cy - scale * (tar_cx * sin_r + tar_cy * cos_r);

    Some((scale, rotation, tx, ty))
}

/// Compute a reasonable position threshold based on star field density.
fn compute_position_threshold(positions: &[(f64, f64)]) -> f64 {
    if positions.len() < 2 {
        return 100.0; // Default large threshold
    }

    // Find average nearest-neighbor distance
    let mut total_min_dist = 0.0;
    for (i, &(xi, yi)) in positions.iter().enumerate().take(20) {
        let mut min_dist_sq = f64::MAX;
        for (j, &(xj, yj)) in positions.iter().enumerate() {
            if i != j {
                let dist_sq = (xi - xj).powi(2) + (yi - yj).powi(2);
                min_dist_sq = min_dist_sq.min(dist_sq);
            }
        }
        total_min_dist += min_dist_sq.sqrt();
    }

    let avg_nn_dist = total_min_dist / positions.len().min(20) as f64;

    // Threshold is a multiple of average nearest-neighbor distance
    (avg_nn_dist * 3.0).max(5.0)
}

/// Convert star matches to point pairs for transformation estimation.
#[allow(clippy::type_complexity)]
#[allow(dead_code)]
pub(crate) fn matches_to_point_pairs(
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

/// Match stars between reference and target images using triangle pattern matching.
///
/// Uses a k-d tree for efficient triangle formation, making it suitable for
/// large star counts (>100 stars).
///
/// # Arguments
/// * `ref_positions` - Reference image star positions
/// * `target_positions` - Target image star positions
/// * `config` - Triangle matching configuration
///
/// # Returns
/// Vector of matched star pairs with confidence scores
///
/// # Example
/// ```rust,ignore
/// use lumos::registration::{TriangleMatchConfig, match_triangles};
///
/// // Star positions from both images (sorted by brightness)
/// let ref_positions = vec![
///     (100.0, 200.0), (300.0, 150.0), (250.0, 400.0),
///     (500.0, 300.0), (150.0, 350.0), (450.0, 100.0),
/// ];
/// let target_positions = vec![
///     (102.0, 198.0), (302.0, 148.0), (252.0, 398.0),
///     (502.0, 298.0), (152.0, 348.0), (452.0, 98.0),
/// ];
///
/// let config = TriangleMatchConfig::default();
/// let matches = match_triangles(&ref_positions, &target_positions, &config);
///
/// for m in &matches {
///     println!("Ref star {} matches target star {} (votes: {})",
///         m.ref_idx, m.target_idx, m.votes);
/// }
/// ```
pub fn match_triangles(
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

    // Vote for star correspondences and resolve conflicts
    let vote_matrix = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &hash_table,
        config,
        n_ref,
        n_target,
    );
    resolve_matches(vote_matrix, n_ref, n_target, config.min_votes)
}
