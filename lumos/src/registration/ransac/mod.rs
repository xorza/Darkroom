//! RANSAC (Random Sample Consensus) for robust transformation estimation.
//!
//! This module implements RANSAC to robustly estimate transformations in the
//! presence of outliers. It works by:
//! 1. Randomly sampling minimal point sets
//! 2. Computing candidate transformations
//! 3. Counting inliers (points within threshold)
//! 4. Keeping the best model
//! 5. Refining with least squares on inliers

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

use crate::registration::types::{TransformMatrix, TransformType};

/// RANSAC configuration.
#[derive(Debug, Clone)]
pub struct RansacConfig {
    /// Maximum iterations.
    pub max_iterations: usize,
    /// Inlier distance threshold in pixels.
    pub inlier_threshold: f64,
    /// Target confidence for early termination.
    pub confidence: f64,
    /// Minimum inlier ratio to accept model.
    pub min_inlier_ratio: f64,
    /// Random seed for reproducibility (None for random).
    pub seed: Option<u64>,
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 1000,
            inlier_threshold: 2.0,
            confidence: 0.999,
            min_inlier_ratio: 0.5,
            seed: None,
        }
    }
}

/// Result of RANSAC estimation.
#[derive(Debug, Clone)]
pub struct RansacResult {
    /// Best transformation found.
    pub transform: TransformMatrix,
    /// Indices of inlier matches.
    pub inliers: Vec<usize>,
    /// Number of iterations performed.
    pub iterations: usize,
    /// Final inlier ratio.
    pub inlier_ratio: f64,
}

/// RANSAC estimator for robust transformation fitting.
pub struct RansacEstimator {
    config: RansacConfig,
}

impl RansacEstimator {
    /// Create a new RANSAC estimator.
    pub fn new(config: RansacConfig) -> Self {
        Self { config }
    }

    /// Estimate transformation from matched point pairs.
    ///
    /// # Arguments
    /// * `ref_points` - Reference (source) point positions
    /// * `target_points` - Target (destination) point positions
    /// * `transform_type` - Type of transformation to estimate
    ///
    /// # Returns
    /// Best transformation found, or None if estimation failed.
    pub fn estimate(
        &self,
        ref_points: &[(f64, f64)],
        target_points: &[(f64, f64)],
        transform_type: TransformType,
    ) -> Option<RansacResult> {
        let n = ref_points.len();
        let min_samples = transform_type.min_points();

        if n < min_samples {
            return None;
        }

        let mut rng: ChaCha8Rng = match self.config.seed {
            Some(seed) => ChaCha8Rng::seed_from_u64(seed),
            None => ChaCha8Rng::from_os_rng(),
        };

        let mut best_transform: Option<TransformMatrix> = None;
        let mut best_inliers: Vec<usize> = Vec::new();
        let mut best_score = 0;

        let mut iterations = 0;
        let max_iter = self.config.max_iterations;

        while iterations < max_iter {
            iterations += 1;

            // Random sample
            let sample_indices = random_sample(&mut rng, n, min_samples);

            // Extract sample points
            let sample_ref: Vec<(f64, f64)> =
                sample_indices.iter().map(|&i| ref_points[i]).collect();
            let sample_target: Vec<(f64, f64)> =
                sample_indices.iter().map(|&i| target_points[i]).collect();

            // Estimate transformation from sample
            let transform = match estimate_transform(&sample_ref, &sample_target, transform_type) {
                Some(t) => t,
                None => continue,
            };

            // Count inliers
            let (inliers, score) = count_inliers(
                ref_points,
                target_points,
                &transform,
                self.config.inlier_threshold,
            );

            // Update best if improved
            if score > best_score {
                best_score = score;
                best_inliers = inliers;
                best_transform = Some(transform);

                // Adaptive iteration count based on inlier ratio
                let inlier_ratio = best_inliers.len() as f64 / n as f64;
                if inlier_ratio >= self.config.min_inlier_ratio {
                    let adaptive_max =
                        adaptive_iterations(inlier_ratio, min_samples, self.config.confidence);
                    if iterations >= adaptive_max {
                        break;
                    }
                }
            }
        }

        // Refine with least squares on inliers
        if let Some(transform) = best_transform
            && best_inliers.len() >= min_samples
        {
            let inlier_ref: Vec<(f64, f64)> = best_inliers.iter().map(|&i| ref_points[i]).collect();
            let inlier_target: Vec<(f64, f64)> =
                best_inliers.iter().map(|&i| target_points[i]).collect();

            let refined =
                refine_transform(&inlier_ref, &inlier_target, transform_type).unwrap_or(transform);

            // Recount inliers with refined transform
            let (final_inliers, _) = count_inliers(
                ref_points,
                target_points,
                &refined,
                self.config.inlier_threshold,
            );

            let inlier_ratio = final_inliers.len() as f64 / n as f64;

            return Some(RansacResult {
                transform: refined,
                inliers: final_inliers,
                iterations,
                inlier_ratio,
            });
        }

        None
    }
}

/// Randomly sample k unique indices from 0..n.
fn random_sample<R: Rng>(rng: &mut R, n: usize, k: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..n).collect();
    indices.shuffle(rng);
    indices.truncate(k);
    indices
}

/// Count inliers and compute score.
fn count_inliers(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    threshold: f64,
) -> (Vec<usize>, usize) {
    let threshold_sq = threshold * threshold;
    let mut inliers = Vec::new();
    let mut score = 0;

    for (i, (&(rx, ry), &(tx, ty))) in ref_points.iter().zip(target_points.iter()).enumerate() {
        let (px, py) = transform.apply(rx, ry);
        let dist_sq = (px - tx).powi(2) + (py - ty).powi(2);

        if dist_sq < threshold_sq {
            inliers.push(i);
            // Use inverse distance as score contribution
            score += ((threshold_sq - dist_sq) * 1000.0) as usize;
        }
    }

    (inliers, score)
}

/// Compute adaptive iteration count for early termination.
pub fn adaptive_iterations(inlier_ratio: f64, sample_size: usize, confidence: f64) -> usize {
    if inlier_ratio <= 0.0 || inlier_ratio >= 1.0 {
        return 1;
    }

    // N = log(1 - confidence) / log(1 - w^n)
    // where w = inlier_ratio, n = sample_size
    let w_n = inlier_ratio.powi(sample_size as i32);
    if w_n >= 1.0 {
        return 1;
    }

    let log_conf = (1.0 - confidence).ln();
    let log_outlier = (1.0 - w_n).ln();

    if log_outlier >= 0.0 {
        return 1000; // Fallback to high number
    }

    (log_conf / log_outlier).ceil() as usize
}

/// Estimate transformation from point correspondences.
pub fn estimate_transform(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform_type: TransformType,
) -> Option<TransformMatrix> {
    match transform_type {
        TransformType::Translation => estimate_translation(ref_points, target_points),
        TransformType::Euclidean => estimate_euclidean(ref_points, target_points),
        TransformType::Similarity => estimate_similarity(ref_points, target_points),
        TransformType::Affine => estimate_affine(ref_points, target_points),
        TransformType::Homography => estimate_homography(ref_points, target_points),
    }
}

/// Estimate translation (average displacement).
fn estimate_translation(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> Option<TransformMatrix> {
    if ref_points.is_empty() {
        return None;
    }

    let mut dx_sum = 0.0;
    let mut dy_sum = 0.0;

    for ((rx, ry), (tx, ty)) in ref_points.iter().zip(target_points.iter()) {
        dx_sum += tx - rx;
        dy_sum += ty - ry;
    }

    let n = ref_points.len() as f64;
    Some(TransformMatrix::translation(dx_sum / n, dy_sum / n))
}

/// Estimate Euclidean transform (translation + rotation).
fn estimate_euclidean(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> Option<TransformMatrix> {
    // Use similarity estimation with scale=1
    let sim = estimate_similarity(ref_points, target_points)?;
    let (dx, dy) = sim.translation_components();
    let angle = sim.rotation_angle();
    Some(TransformMatrix::euclidean(dx, dy, angle))
}

/// Estimate similarity transform (translation + rotation + uniform scale).
pub fn estimate_similarity(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> Option<TransformMatrix> {
    if ref_points.len() < 2 {
        return None;
    }

    // Compute centroids
    let (ref_cx, ref_cy) = centroid(ref_points);
    let (tar_cx, tar_cy) = centroid(target_points);

    // Center the points
    let ref_centered: Vec<(f64, f64)> = ref_points
        .iter()
        .map(|(x, y)| (x - ref_cx, y - ref_cy))
        .collect();
    let tar_centered: Vec<(f64, f64)> = target_points
        .iter()
        .map(|(x, y)| (x - tar_cx, y - tar_cy))
        .collect();

    // Compute covariance terms
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syx = 0.0;
    let mut syy = 0.0;
    let mut ref_var = 0.0;

    for ((rx, ry), (tx, ty)) in ref_centered.iter().zip(tar_centered.iter()) {
        sxx += rx * tx;
        sxy += rx * ty;
        syx += ry * tx;
        syy += ry * ty;
        ref_var += rx * rx + ry * ry;
    }

    if ref_var < 1e-10 {
        return None;
    }

    // Compute rotation angle
    let angle = (sxy - syx).atan2(sxx + syy);

    // Compute scale
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let scale = ((sxx + syy) * cos_a + (sxy - syx) * sin_a) / ref_var;

    if scale <= 0.0 {
        return None;
    }

    // Compute translation
    let dx = tar_cx - scale * (cos_a * ref_cx - sin_a * ref_cy);
    let dy = tar_cy - scale * (sin_a * ref_cx + cos_a * ref_cy);

    Some(TransformMatrix::similarity(dx, dy, angle, scale))
}

/// Estimate affine transform using least squares.
pub fn estimate_affine(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> Option<TransformMatrix> {
    if ref_points.len() < 3 {
        return None;
    }

    // Solve: target = A * ref + b
    // In matrix form: [tx] = [a b] [rx] + [e]
    //                 [ty]   [c d] [ry]   [f]
    //
    // We solve two systems: one for x-coordinates, one for y-coordinates
    // Using normal equations: A^T A x = A^T b

    let n = ref_points.len() as f64;

    // Compute sums
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_yy = 0.0;
    let mut sum_tx = 0.0;
    let mut sum_ty = 0.0;
    let mut sum_x_tx = 0.0;
    let mut sum_y_tx = 0.0;
    let mut sum_x_ty = 0.0;
    let mut sum_y_ty = 0.0;

    for ((rx, ry), (tx, ty)) in ref_points.iter().zip(target_points.iter()) {
        sum_x += rx;
        sum_y += ry;
        sum_xx += rx * rx;
        sum_xy += rx * ry;
        sum_yy += ry * ry;
        sum_tx += tx;
        sum_ty += ty;
        sum_x_tx += rx * tx;
        sum_y_tx += ry * tx;
        sum_x_ty += rx * ty;
        sum_y_ty += ry * ty;
    }

    // Build and solve 3x3 system for each target coordinate
    // [sum_xx  sum_xy  sum_x ] [a]   [sum_x_tx]
    // [sum_xy  sum_yy  sum_y ] [b] = [sum_y_tx]
    // [sum_x   sum_y   n     ] [e]   [sum_tx  ]

    let det = sum_xx * (sum_yy * n - sum_y * sum_y) - sum_xy * (sum_xy * n - sum_y * sum_x)
        + sum_x * (sum_xy * sum_y - sum_yy * sum_x);

    if det.abs() < 1e-10 {
        return None;
    }

    let inv_det = 1.0 / det;

    // Compute inverse of the matrix (Cramer's rule would work too)
    let m00 = (sum_yy * n - sum_y * sum_y) * inv_det;
    let m01 = (sum_x * sum_y - sum_xy * n) * inv_det;
    let m02 = (sum_xy * sum_y - sum_yy * sum_x) * inv_det;
    let m10 = (sum_y * sum_x - sum_xy * n) * inv_det;
    let m11 = (sum_xx * n - sum_x * sum_x) * inv_det;
    let m12 = (sum_xy * sum_x - sum_xx * sum_y) * inv_det;
    let m20 = (sum_xy * sum_y - sum_x * sum_yy) * inv_det;
    let m21 = (sum_xy * sum_x - sum_y * sum_xx) * inv_det;
    let m22 = (sum_xx * sum_yy - sum_xy * sum_xy) * inv_det;

    // Solve for x-coordinate parameters
    let a = m00 * sum_x_tx + m01 * sum_y_tx + m02 * sum_tx;
    let b = m10 * sum_x_tx + m11 * sum_y_tx + m12 * sum_tx;
    let e = m20 * sum_x_tx + m21 * sum_y_tx + m22 * sum_tx;

    // Solve for y-coordinate parameters
    let c = m00 * sum_x_ty + m01 * sum_y_ty + m02 * sum_ty;
    let d = m10 * sum_x_ty + m11 * sum_y_ty + m12 * sum_ty;
    let f = m20 * sum_x_ty + m21 * sum_y_ty + m22 * sum_ty;

    let transform = TransformMatrix::affine([a, b, e, c, d, f]);

    if transform.is_valid() {
        Some(transform)
    } else {
        None
    }
}

/// Estimate homography using Direct Linear Transform (DLT).
pub fn estimate_homography(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> Option<TransformMatrix> {
    if ref_points.len() < 4 {
        return None;
    }

    // For simplicity, if we have exactly the minimum, use a simpler closed-form solution
    // For overdetermined systems, use least squares via SVD

    // Normalize points for numerical stability
    let (ref_norm, ref_t) = normalize_points(ref_points);
    let (tar_norm, tar_t) = normalize_points(target_points);

    // Build the DLT matrix A where Ah = 0
    // Each point gives 2 equations:
    // [-x -y -1  0  0  0  x*x'  y*x'  x']
    // [ 0  0  0 -x -y -1  x*y'  y*y'  y']

    let n = ref_norm.len();
    let mut ata = [[0.0f64; 9]; 9];

    for i in 0..n {
        let (x, y) = ref_norm[i];
        let (xp, yp) = tar_norm[i];

        let row1 = [-x, -y, -1.0, 0.0, 0.0, 0.0, x * xp, y * xp, xp];
        let row2 = [0.0, 0.0, 0.0, -x, -y, -1.0, x * yp, y * yp, yp];

        // Add to A^T A
        for j in 0..9 {
            for k in 0..9 {
                ata[j][k] += row1[j] * row1[k] + row2[j] * row2[k];
            }
        }
    }

    // Find eigenvector corresponding to smallest eigenvalue using power iteration
    // (simplified approach - for production, use proper SVD)
    let h = solve_homogeneous_9x9(&ata)?;

    // Denormalize: H = T_target^-1 * H_norm * T_ref
    let h_norm = TransformMatrix::from_matrix(h, TransformType::Homography);
    let tar_t_inv = tar_t.inverse(); // Normalization transforms are always invertible

    let h_denorm = tar_t_inv.compose(&h_norm).compose(&ref_t);

    // Normalize so h[8] = 1
    let scale = h_denorm.data[8];
    if scale.abs() < 1e-10 {
        return None;
    }

    let mut data = h_denorm.data;
    for d in &mut data {
        *d /= scale;
    }

    let result = TransformMatrix::from_matrix(data, TransformType::Homography);

    if result.is_valid() {
        Some(result)
    } else {
        None
    }
}

/// Normalize points for numerical stability.
pub fn normalize_points(points: &[(f64, f64)]) -> (Vec<(f64, f64)>, TransformMatrix) {
    if points.is_empty() {
        return (Vec::new(), TransformMatrix::identity());
    }

    // Compute centroid
    let (cx, cy) = centroid(points);

    // Compute average distance from centroid
    let mut avg_dist = 0.0;
    for &(x, y) in points {
        avg_dist += ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    }
    avg_dist /= points.len() as f64;

    if avg_dist < 1e-10 {
        return (points.to_vec(), TransformMatrix::identity());
    }

    // Scale so average distance is sqrt(2)
    let scale = std::f64::consts::SQRT_2 / avg_dist;

    let normalized: Vec<(f64, f64)> = points
        .iter()
        .map(|&(x, y)| ((x - cx) * scale, (y - cy) * scale))
        .collect();

    // Transformation matrix: translate then scale
    let t = TransformMatrix::from_matrix(
        [
            scale,
            0.0,
            -cx * scale,
            0.0,
            scale,
            -cy * scale,
            0.0,
            0.0,
            1.0,
        ],
        TransformType::Affine,
    );

    (normalized, t)
}

/// Solve 9x9 homogeneous system using inverse iteration.
fn solve_homogeneous_9x9(ata: &[[f64; 9]; 9]) -> Option<[f64; 9]> {
    // Use inverse iteration to find eigenvector of smallest eigenvalue
    // Start with random vector and iterate

    let mut v = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let epsilon = 1e-10;

    // Add small regularization to diagonal for invertibility
    let mut a_reg = *ata;
    #[allow(clippy::needless_range_loop)]
    for i in 0..9 {
        a_reg[i][i] += epsilon;
    }

    // Inverse iteration: v = A^-1 * v, then normalize
    for _ in 0..50 {
        // Solve A * v_new = v using Gaussian elimination
        let v_new = solve_linear_9x9(&a_reg, &v)?;

        // Normalize
        let norm: f64 = v_new.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < 1e-15 {
            return None;
        }

        for i in 0..9 {
            v[i] = v_new[i] / norm;
        }
    }

    Some(v)
}

/// Solve 9x9 linear system using Gaussian elimination with partial pivoting.
fn solve_linear_9x9(a: &[[f64; 9]; 9], b: &[f64; 9]) -> Option<[f64; 9]> {
    let mut aug = [[0.0; 10]; 9];

    // Build augmented matrix
    for i in 0..9 {
        for j in 0..9 {
            aug[i][j] = a[i][j];
        }
        aug[i][9] = b[i];
    }

    // Forward elimination with partial pivoting
    #[allow(clippy::needless_range_loop)]
    for col in 0..9 {
        // Find pivot
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..9 {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }

        if max_val < 1e-15 {
            return None; // Singular matrix
        }

        // Swap rows
        if max_row != col {
            aug.swap(col, max_row);
        }

        // Eliminate
        for row in (col + 1)..9 {
            let factor = aug[row][col] / aug[col][col];
            for j in col..10 {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    let mut x = [0.0; 9];
    for i in (0..9).rev() {
        x[i] = aug[i][9];
        for j in (i + 1)..9 {
            x[i] -= aug[i][j] * x[j];
        }
        x[i] /= aug[i][i];
    }

    Some(x)
}

/// Compute centroid of points.
pub fn centroid(points: &[(f64, f64)]) -> (f64, f64) {
    if points.is_empty() {
        return (0.0, 0.0);
    }

    let mut cx = 0.0;
    let mut cy = 0.0;
    for &(x, y) in points {
        cx += x;
        cy += y;
    }
    let n = points.len() as f64;
    (cx / n, cy / n)
}

/// Refine transformation using all inlier points.
pub fn refine_transform(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform_type: TransformType,
) -> Option<TransformMatrix> {
    estimate_transform(ref_points, target_points, transform_type)
}

/// Compute residuals for a transformation.
pub fn compute_residuals(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
) -> Vec<f64> {
    ref_points
        .iter()
        .zip(target_points.iter())
        .map(|(&(rx, ry), &(tx, ty))| {
            let (px, py) = transform.apply(rx, ry);
            ((px - tx).powi(2) + (py - ty).powi(2)).sqrt()
        })
        .collect()
}
