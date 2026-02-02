//! Registration result types.

use crate::registration::types::TransformMatrix;

/// Result of image registration.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    /// Computed transformation matrix.
    pub transform: TransformMatrix,

    /// Matched star pairs as (reference_idx, target_idx).
    pub matched_stars: Vec<(usize, usize)>,

    /// Per-match residuals in pixels.
    pub residuals: Vec<f64>,

    /// RMS registration error in pixels.
    pub rms_error: f64,

    /// Maximum residual error in pixels.
    pub max_error: f64,

    /// Number of RANSAC inliers.
    pub num_inliers: usize,

    /// Registration quality score (0.0 - 1.0).
    pub quality_score: f64,

    /// Processing time in milliseconds.
    pub elapsed_ms: f64,
}

impl RegistrationResult {
    /// Create a new registration result.
    pub fn new(
        transform: TransformMatrix,
        matched_stars: Vec<(usize, usize)>,
        residuals: Vec<f64>,
    ) -> Self {
        let rms_error = if residuals.is_empty() {
            0.0
        } else {
            let sum_sq: f64 = residuals.iter().map(|r| r * r).sum();
            (sum_sq / residuals.len() as f64).sqrt()
        };

        let max_error = residuals
            .iter()
            .copied()
            .fold(0.0, |a, b| if a > b { a } else { b });

        let num_inliers = matched_stars.len();

        // Simple quality score based on RMS error and match count
        let quality_score = if num_inliers < 4 {
            0.0
        } else {
            let error_factor = (-rms_error / 2.0).exp();
            let count_factor = (num_inliers as f64 / 20.0).min(1.0);
            error_factor * count_factor
        };

        Self {
            transform,
            matched_stars,
            residuals,
            rms_error,
            max_error,
            num_inliers,
            quality_score,
            elapsed_ms: 0.0,
        }
    }

    /// Set the elapsed time.
    pub fn with_elapsed(mut self, ms: f64) -> Self {
        self.elapsed_ms = ms;
        self
    }
}
