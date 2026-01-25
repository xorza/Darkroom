//! Registration quality metrics and validation.
//!
//! Provides tools for assessing alignment quality, detecting misregistrations,
//! and computing overlap statistics.

use crate::registration::types::TransformMatrix;

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

/// Quality assessment result for image registration.
#[derive(Debug, Clone)]
pub struct QualityMetrics {
    /// Root mean square error of residuals (pixels).
    pub rms_error: f64,
    /// Maximum residual error (pixels).
    pub max_error: f64,
    /// Median residual error (pixels).
    pub median_error: f64,
    /// Number of matched star pairs.
    pub num_matches: usize,
    /// Number of inliers after RANSAC.
    pub num_inliers: usize,
    /// Inlier ratio (inliers / total matches).
    pub inlier_ratio: f64,
    /// Overall quality score (0.0 - 1.0).
    pub quality_score: f64,
    /// Estimated overlap fraction between images.
    pub overlap_fraction: f64,
    /// Indicates if registration is considered successful.
    pub is_valid: bool,
    /// Reason if registration is invalid.
    pub failure_reason: Option<String>,
}

impl QualityMetrics {
    /// Compute quality metrics from residuals and match counts.
    pub fn compute(
        residuals: &[f64],
        num_matches: usize,
        num_inliers: usize,
        overlap_fraction: f64,
    ) -> Self {
        if residuals.is_empty() {
            return Self::invalid("No residuals to compute");
        }

        let rms_error = {
            let sum_sq: f64 = residuals.iter().map(|r| r * r).sum();
            (sum_sq / residuals.len() as f64).sqrt()
        };

        let max_error = residuals.iter().copied().fold(0.0, f64::max);

        let median_error = {
            let mut sorted = residuals.to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mid = sorted.len() / 2;
            if sorted.len().is_multiple_of(2) {
                (sorted[mid - 1] + sorted[mid]) / 2.0
            } else {
                sorted[mid]
            }
        };

        let inlier_ratio = if num_matches > 0 {
            num_inliers as f64 / num_matches as f64
        } else {
            0.0
        };

        // Compute quality score based on multiple factors
        let quality_score =
            compute_quality_score(rms_error, num_inliers, inlier_ratio, overlap_fraction);

        // Determine if registration is valid
        let (is_valid, failure_reason) =
            validate_registration(rms_error, num_inliers, inlier_ratio);

        Self {
            rms_error,
            max_error,
            median_error,
            num_matches,
            num_inliers,
            inlier_ratio,
            quality_score,
            overlap_fraction,
            is_valid,
            failure_reason,
        }
    }

    /// Create metrics for an invalid registration.
    fn invalid(reason: &str) -> Self {
        Self {
            rms_error: f64::INFINITY,
            max_error: f64::INFINITY,
            median_error: f64::INFINITY,
            num_matches: 0,
            num_inliers: 0,
            inlier_ratio: 0.0,
            quality_score: 0.0,
            overlap_fraction: 0.0,
            is_valid: false,
            failure_reason: Some(reason.to_string()),
        }
    }
}

/// Compute overall quality score (0.0 - 1.0).
fn compute_quality_score(
    rms_error: f64,
    num_inliers: usize,
    inlier_ratio: f64,
    overlap_fraction: f64,
) -> f64 {
    // Error component: exponential decay, 0.5 pixel = 1.0, 2.0 pixels = 0.37
    let error_score = (-rms_error / 2.0).exp();

    // Match count component: saturates at ~50 matches
    let match_score = (num_inliers as f64 / 50.0).min(1.0);

    // Inlier ratio component: linear, penalizes many outliers
    let inlier_score = inlier_ratio;

    // Overlap component: full overlap = 1.0
    let overlap_score = overlap_fraction;

    // Weighted combination
    let score = 0.4 * error_score + 0.25 * match_score + 0.2 * inlier_score + 0.15 * overlap_score;

    score.clamp(0.0, 1.0)
}

/// Validate registration and return (is_valid, reason).
fn validate_registration(
    rms_error: f64,
    num_inliers: usize,
    inlier_ratio: f64,
) -> (bool, Option<String>) {
    const MIN_INLIERS: usize = 4;
    const MAX_RMS_ERROR: f64 = 5.0;
    const MIN_INLIER_RATIO: f64 = 0.3;

    if num_inliers < MIN_INLIERS {
        return (
            false,
            Some(format!(
                "Too few inliers: {} < {}",
                num_inliers, MIN_INLIERS
            )),
        );
    }

    if rms_error > MAX_RMS_ERROR {
        return (
            false,
            Some(format!(
                "RMS error too high: {:.2} > {:.2}",
                rms_error, MAX_RMS_ERROR
            )),
        );
    }

    if inlier_ratio < MIN_INLIER_RATIO {
        return (
            false,
            Some(format!(
                "Low inlier ratio: {:.2} < {:.2}",
                inlier_ratio, MIN_INLIER_RATIO
            )),
        );
    }

    (true, None)
}

/// Estimate overlap fraction between two images after transformation.
///
/// # Arguments
///
/// * `width` - Image width
/// * `height` - Image height
/// * `transform` - Transform from reference to target
///
/// # Returns
///
/// Estimated fraction of reference image that overlaps with target (0.0 - 1.0).
pub fn estimate_overlap(width: usize, height: usize, transform: &TransformMatrix) -> f64 {
    // Sample corners and compute bounding box of transformed reference in target space
    let corners = [
        (0.0, 0.0),
        (width as f64, 0.0),
        (width as f64, height as f64),
        (0.0, height as f64),
    ];

    let transformed: Vec<_> = corners
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
        .collect();

    // Compute bounding box of transformed corners
    let min_x = transformed
        .iter()
        .map(|p| p.0)
        .fold(f64::INFINITY, f64::min);
    let max_x = transformed
        .iter()
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = transformed
        .iter()
        .map(|p| p.1)
        .fold(f64::INFINITY, f64::min);
    let max_y = transformed
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max);

    // Compute overlap with target image bounds
    let overlap_min_x = min_x.max(0.0);
    let overlap_max_x = max_x.min(width as f64);
    let overlap_min_y = min_y.max(0.0);
    let overlap_max_y = max_y.min(height as f64);

    if overlap_max_x <= overlap_min_x || overlap_max_y <= overlap_min_y {
        return 0.0;
    }

    let overlap_area = (overlap_max_x - overlap_min_x) * (overlap_max_y - overlap_min_y);
    let total_area = (width * height) as f64;

    (overlap_area / total_area).clamp(0.0, 1.0)
}

/// Compute per-star residuals given transformation.
pub fn compute_residuals(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
) -> Vec<f64> {
    assert_eq!(
        ref_points.len(),
        target_points.len(),
        "Point count mismatch"
    );

    ref_points
        .iter()
        .zip(target_points.iter())
        .map(|(&(rx, ry), &(tx, ty))| {
            let (px, py) = transform.apply(rx, ry);
            ((px - tx).powi(2) + (py - ty).powi(2)).sqrt()
        })
        .collect()
}

/// Compute registration statistics for a set of residuals.
#[derive(Debug, Clone)]
pub struct ResidualStats {
    pub mean: f64,
    pub rms: f64,
    pub median: f64,
    pub std_dev: f64,
    pub min: f64,
    pub max: f64,
    pub percentile_90: f64,
    pub percentile_95: f64,
}

impl ResidualStats {
    /// Compute statistics from residuals.
    pub fn compute(residuals: &[f64]) -> Self {
        if residuals.is_empty() {
            return Self {
                mean: 0.0,
                rms: 0.0,
                median: 0.0,
                std_dev: 0.0,
                min: 0.0,
                max: 0.0,
                percentile_90: 0.0,
                percentile_95: 0.0,
            };
        }

        let n = residuals.len() as f64;

        let mean = residuals.iter().sum::<f64>() / n;

        let rms = {
            let sum_sq: f64 = residuals.iter().map(|r| r * r).sum();
            (sum_sq / n).sqrt()
        };

        let mut sorted = residuals.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let median = {
            let mid = sorted.len() / 2;
            if sorted.len().is_multiple_of(2) {
                (sorted[mid - 1] + sorted[mid]) / 2.0
            } else {
                sorted[mid]
            }
        };

        let variance = residuals.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
        let std_dev = variance.sqrt();

        let min = sorted[0];
        let max = sorted[sorted.len() - 1];

        let percentile_90 = percentile(&sorted, 0.90);
        let percentile_95 = percentile(&sorted, 0.95);

        Self {
            mean,
            rms,
            median,
            std_dev,
            min,
            max,
            percentile_90,
            percentile_95,
        }
    }
}

/// Compute percentile value from sorted data.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Check if registration is consistent across image quadrants.
///
/// This helps detect cases where registration is only correct in part of the image.
pub fn check_quadrant_consistency(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    width: usize,
    height: usize,
) -> QuadrantConsistency {
    let half_w = width as f64 / 2.0;
    let half_h = height as f64 / 2.0;

    let mut quadrant_residuals: [Vec<f64>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for (&(rx, ry), &(tx, ty)) in ref_points.iter().zip(target_points.iter()) {
        let (px, py) = transform.apply(rx, ry);
        let residual = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();

        // Determine quadrant (0=TL, 1=TR, 2=BL, 3=BR)
        let q = match (rx >= half_w, ry >= half_h) {
            (false, false) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (true, true) => 3,
        };

        quadrant_residuals[q].push(residual);
    }

    let quadrant_rms: Vec<f64> = quadrant_residuals
        .iter()
        .map(|r| {
            if r.is_empty() {
                f64::INFINITY
            } else {
                let sum_sq: f64 = r.iter().map(|v| v * v).sum();
                (sum_sq / r.len() as f64).sqrt()
            }
        })
        .collect();

    let quadrant_counts: Vec<usize> = quadrant_residuals.iter().map(|r| r.len()).collect();

    // Check consistency
    let valid_rms: Vec<f64> = quadrant_rms
        .iter()
        .filter(|&&r| r.is_finite())
        .copied()
        .collect();
    let (is_consistent, max_difference) = if valid_rms.len() >= 2 {
        let min_rms = valid_rms.iter().copied().fold(f64::INFINITY, f64::min);
        let max_rms = valid_rms.iter().copied().fold(0.0, f64::max);
        let diff = max_rms - min_rms;
        // Consider consistent if difference < 2 pixels
        (diff < 2.0, diff)
    } else {
        (true, 0.0)
    };

    QuadrantConsistency {
        quadrant_rms: [
            quadrant_rms[0],
            quadrant_rms[1],
            quadrant_rms[2],
            quadrant_rms[3],
        ],
        quadrant_counts: [
            quadrant_counts[0],
            quadrant_counts[1],
            quadrant_counts[2],
            quadrant_counts[3],
        ],
        is_consistent,
        max_difference,
    }
}

/// Quadrant-wise registration consistency analysis.
#[derive(Debug, Clone)]
pub struct QuadrantConsistency {
    /// RMS error per quadrant [TL, TR, BL, BR].
    pub quadrant_rms: [f64; 4],
    /// Point count per quadrant.
    pub quadrant_counts: [usize; 4],
    /// Whether registration is consistent across quadrants.
    pub is_consistent: bool,
    /// Maximum RMS difference between quadrants.
    pub max_difference: f64,
}
