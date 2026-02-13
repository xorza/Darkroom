//! Transform estimation functions for RANSAC.
//!
//! Pure geometry / linear algebra: translation, euclidean, similarity, affine,
//! and homography estimation from point correspondences.

use glam::DVec2;
use nalgebra::{DMatrix, SVD};

use crate::math::DMat3;
use crate::registration::transform::{Transform, TransformType};

/// Compute adaptive iteration count for early termination.
pub(crate) fn adaptive_iterations(inlier_ratio: f64, sample_size: usize, confidence: f64) -> usize {
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
pub(crate) fn estimate_transform(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform_type: TransformType,
) -> Option<Transform> {
    match transform_type {
        TransformType::Translation => estimate_translation(ref_points, target_points),
        TransformType::Euclidean => estimate_euclidean(ref_points, target_points),
        TransformType::Similarity => estimate_similarity(ref_points, target_points),
        TransformType::Affine => estimate_affine(ref_points, target_points),
        TransformType::Homography => estimate_homography(ref_points, target_points),
        TransformType::Auto => {
            panic!("Auto must be resolved to a concrete type before calling estimate_transform")
        }
    }
}

/// Estimate translation (average displacement).
fn estimate_translation(ref_points: &[DVec2], target_points: &[DVec2]) -> Option<Transform> {
    if ref_points.is_empty() {
        return None;
    }

    let mut d_sum = DVec2::ZERO;

    for (r, t) in ref_points.iter().zip(target_points.iter()) {
        d_sum += *t - *r;
    }

    let n = ref_points.len() as f64;
    Some(Transform::translation(d_sum / n))
}

/// Estimate Euclidean transform (translation + rotation, scale fixed at 1.0).
///
/// Uses constrained Procrustes analysis: computes optimal rotation from the
/// cross-covariance matrix of centered points, then derives translation with
/// scale=1. Unlike similarity estimation, scale is never fitted.
fn estimate_euclidean(ref_points: &[DVec2], target_points: &[DVec2]) -> Option<Transform> {
    if ref_points.len() < 2 {
        return None;
    }

    let ref_centroid = centroid(ref_points);
    let tar_centroid = centroid(target_points);

    // Cross-covariance terms (no ref_var needed since scale=1)
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syx = 0.0;
    let mut syy = 0.0;
    for (r, t) in ref_points.iter().zip(target_points.iter()) {
        let rc = *r - ref_centroid;
        let tc = *t - tar_centroid;
        sxx += rc.x * tc.x;
        sxy += rc.x * tc.y;
        syx += rc.y * tc.x;
        syy += rc.y * tc.y;
    }

    let angle = (sxy - syx).atan2(sxx + syy);
    let (sin_a, cos_a) = angle.sin_cos();

    // Translation with scale=1
    let t = DVec2::new(
        tar_centroid.x - (cos_a * ref_centroid.x - sin_a * ref_centroid.y),
        tar_centroid.y - (sin_a * ref_centroid.x + cos_a * ref_centroid.y),
    );

    Some(Transform::euclidean(t, angle))
}

/// Estimate similarity transform (translation + rotation + uniform scale).
pub(crate) fn estimate_similarity(
    ref_points: &[DVec2],
    target_points: &[DVec2],
) -> Option<Transform> {
    if ref_points.len() < 2 {
        return None;
    }

    // Compute centroids
    let ref_centroid = centroid(ref_points);
    let tar_centroid = centroid(target_points);

    // Center the points
    let ref_centered: Vec<DVec2> = ref_points.iter().map(|p| *p - ref_centroid).collect();
    let tar_centered: Vec<DVec2> = target_points.iter().map(|p| *p - tar_centroid).collect();

    // Compute covariance terms
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syx = 0.0;
    let mut syy = 0.0;
    let mut ref_var = 0.0;

    for (r, t) in ref_centered.iter().zip(tar_centered.iter()) {
        sxx += r.x * t.x;
        sxy += r.x * t.y;
        syx += r.y * t.x;
        syy += r.y * t.y;
        ref_var += r.length_squared();
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
    let t = DVec2::new(
        tar_centroid.x - scale * (cos_a * ref_centroid.x - sin_a * ref_centroid.y),
        tar_centroid.y - scale * (sin_a * ref_centroid.x + cos_a * ref_centroid.y),
    );

    Some(Transform::similarity(t, angle, scale))
}

/// Estimate affine transform using least squares.
pub(crate) fn estimate_affine(ref_points: &[DVec2], target_points: &[DVec2]) -> Option<Transform> {
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

    for (r, t) in ref_points.iter().zip(target_points.iter()) {
        sum_x += r.x;
        sum_y += r.y;
        sum_xx += r.x * r.x;
        sum_xy += r.x * r.y;
        sum_yy += r.y * r.y;
        sum_tx += t.x;
        sum_ty += t.y;
        sum_x_tx += r.x * t.x;
        sum_y_tx += r.y * t.x;
        sum_x_ty += r.x * t.y;
        sum_y_ty += r.y * t.y;
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

    let transform = Transform::affine([a, b, e, c, d, f]);

    if transform.is_valid() {
        Some(transform)
    } else {
        None
    }
}

/// Estimate homography using Direct Linear Transform (DLT).
pub(crate) fn estimate_homography(
    ref_points: &[DVec2],
    target_points: &[DVec2],
) -> Option<Transform> {
    if ref_points.len() < 4 {
        return None;
    }

    // Normalize points for numerical stability
    let (ref_norm, ref_t) = normalize_points(ref_points);
    let (tar_norm, tar_t) = normalize_points(target_points);

    // Build the DLT matrix A where Ah = 0
    // Each point gives 2 equations:
    // [-x -y -1  0  0  0  x*x'  y*x'  x']
    // [ 0  0  0 -x -y -1  x*y'  y*y'  y']

    let n = ref_norm.len();

    // Build the full 2n×9 design matrix A directly (instead of A^T A)
    // to preserve condition number (κ instead of κ²)
    let mut a_data = vec![0.0f64; 2 * n * 9];
    for i in 0..n {
        let r = ref_norm[i];
        let t = tar_norm[i];
        let base = i * 2 * 9;
        a_data[base..base + 9].copy_from_slice(&[
            -r.x,
            -r.y,
            -1.0,
            0.0,
            0.0,
            0.0,
            r.x * t.x,
            r.y * t.x,
            t.x,
        ]);
        a_data[base + 9..base + 18].copy_from_slice(&[
            0.0,
            0.0,
            0.0,
            -r.x,
            -r.y,
            -1.0,
            r.x * t.y,
            r.y * t.y,
            t.y,
        ]);
    }
    let a = DMatrix::from_row_slice(2 * n, 9, &a_data);

    let h = solve_homogeneous_svd(a)?;

    // Denormalize: H = T_target^-1 * H_norm * T_ref
    let h_norm = Transform::from_matrix(h, TransformType::Homography);
    let tar_t_inv = tar_t.inverse(); // Normalization transforms are always invertible

    let h_denorm = tar_t_inv.compose(&h_norm).compose(&ref_t);

    // Normalize so h[8] = 1
    let scale = h_denorm.matrix[8];
    if scale.abs() < 1e-10 {
        return None;
    }

    let mut data = h_denorm.matrix.to_array();
    for d in &mut data {
        *d /= scale;
    }

    let result = Transform::from_matrix(data.into(), TransformType::Homography);

    if result.is_valid() {
        Some(result)
    } else {
        None
    }
}

/// Normalize points for numerical stability.
pub(crate) fn normalize_points(points: &[DVec2]) -> (Vec<DVec2>, Transform) {
    if points.is_empty() {
        return (Vec::new(), Transform::identity());
    }

    // Compute centroid
    let c = centroid(points);

    // Compute average distance from centroid
    let mut avg_dist = 0.0;
    for p in points {
        avg_dist += (*p - c).length();
    }
    avg_dist /= points.len() as f64;

    if avg_dist < 1e-10 {
        return (points.to_vec(), Transform::identity());
    }

    // Scale so average distance is sqrt(2)
    let scale = std::f64::consts::SQRT_2 / avg_dist;

    let normalized: Vec<DVec2> = points.iter().map(|p| (*p - c) * scale).collect();

    // Transformation matrix: translate then scale
    let t = Transform::from_matrix(
        [
            scale,
            0.0,
            -c.x * scale,
            0.0,
            scale,
            -c.y * scale,
            0.0,
            0.0,
            1.0,
        ]
        .into(),
        TransformType::Affine,
    );

    (normalized, t)
}

/// Solve homogeneous system Ah=0 via direct SVD of the m×9 design matrix.
///
/// Returns the right singular vector corresponding to the smallest singular value
/// (last row of V^T). Using the full rectangular matrix preserves condition number κ,
/// vs κ² when SVD-ing A^T A.
fn solve_homogeneous_svd(a: DMatrix<f64>) -> Option<DMat3> {
    let nrows = a.nrows();
    let ncols = a.ncols();

    // nalgebra computes thin SVD: V^T has min(m,n) × n shape.
    // For m < 9, we'd miss the null-space vector (row 8 of full V^T).
    // Pad with zero rows so m >= 9 — zeros don't affect the null space.
    let a = if nrows < ncols {
        let mut padded = DMatrix::zeros(ncols, ncols);
        padded.view_mut((0, 0), (nrows, ncols)).copy_from(&a);
        padded
    } else {
        a
    };

    // Compute SVD — only need V (right singular vectors), skip U
    let svd = SVD::new(a, false, true);

    // Get V^T (right singular vectors as rows, 9×9)
    let v_t = svd.v_t?;

    // The last row of V^T corresponds to the smallest singular value
    let last_row = v_t.row(8);

    let mut data = [0.0f64; 9];
    for (i, &val) in last_row.iter().enumerate() {
        data[i] = val;
    }

    Some(DMat3::from_array(data))
}

/// Compute centroid of points.
pub(crate) fn centroid(points: &[DVec2]) -> DVec2 {
    if points.is_empty() {
        return DVec2::ZERO;
    }

    let mut sum = DVec2::ZERO;
    for p in points {
        sum += *p;
    }
    sum / points.len() as f64
}
