//! Detection metrics computation for visual tests.

use super::comparison::{MatchResult, match_stars};
use crate::star_detection::Star;
use crate::star_detection::visual_tests::generators::GroundTruthStar;
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Comprehensive detection metrics.
#[derive(Debug, Clone)]
pub struct DetectionMetrics {
    // Counts
    /// Number of correctly detected stars (within match radius of ground truth)
    pub true_positives: usize,
    /// Number of spurious detections (no matching ground truth)
    pub false_positives: usize,
    /// Number of missed ground truth stars
    pub false_negatives: usize,
    /// Total ground truth stars
    pub total_truth: usize,
    /// Total detected stars
    pub total_detected: usize,

    // Rates
    /// Detection rate: TP / (TP + FN)
    pub detection_rate: f32,
    /// Precision: TP / (TP + FP)
    pub precision: f32,
    /// F1 score: harmonic mean of detection rate and precision
    pub f1_score: f32,
    /// False positive rate: FP / total_detected
    pub false_positive_rate: f32,

    // Positional accuracy
    /// Centroid errors for matched stars (in pixels)
    pub centroid_errors: Vec<f32>,
    /// Mean centroid error
    pub mean_centroid_error: f32,
    /// Median centroid error
    pub median_centroid_error: f32,
    /// Maximum centroid error
    pub max_centroid_error: f32,
    /// Standard deviation of centroid errors
    pub std_centroid_error: f32,

    // Property accuracy (for matched stars)
    /// FWHM relative errors: (detected - true) / true
    pub fwhm_errors: Vec<f32>,
    /// Mean FWHM error
    pub mean_fwhm_error: f32,
    /// Flux relative errors
    pub flux_errors: Vec<f32>,
    /// Mean flux error
    pub mean_flux_error: f32,

    // Detailed match information
    /// Match result for detailed analysis
    pub match_result: Option<MatchResult>,
}

impl Default for DetectionMetrics {
    fn default() -> Self {
        Self {
            true_positives: 0,
            false_positives: 0,
            false_negatives: 0,
            total_truth: 0,
            total_detected: 0,
            detection_rate: 0.0,
            precision: 0.0,
            f1_score: 0.0,
            false_positive_rate: 0.0,
            centroid_errors: Vec::new(),
            mean_centroid_error: 0.0,
            median_centroid_error: 0.0,
            max_centroid_error: 0.0,
            std_centroid_error: 0.0,
            fwhm_errors: Vec::new(),
            mean_fwhm_error: 0.0,
            flux_errors: Vec::new(),
            mean_flux_error: 0.0,
            match_result: None,
        }
    }
}

impl fmt::Display for DetectionMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Detection Metrics")?;
        writeln!(f, "================")?;
        writeln!(f)?;
        writeln!(f, "Counts:")?;
        writeln!(f, "  Ground truth stars:  {}", self.total_truth)?;
        writeln!(f, "  Detected stars:      {}", self.total_detected)?;
        writeln!(f, "  True positives:      {}", self.true_positives)?;
        writeln!(f, "  False positives:     {}", self.false_positives)?;
        writeln!(f, "  False negatives:     {}", self.false_negatives)?;
        writeln!(f)?;
        writeln!(f, "Rates:")?;
        writeln!(
            f,
            "  Detection rate:      {:.1}%",
            self.detection_rate * 100.0
        )?;
        writeln!(f, "  Precision:           {:.1}%", self.precision * 100.0)?;
        writeln!(f, "  F1 score:            {:.3}", self.f1_score)?;
        writeln!(
            f,
            "  False positive rate: {:.1}%",
            self.false_positive_rate * 100.0
        )?;
        writeln!(f)?;
        writeln!(f, "Centroid Accuracy (pixels):")?;
        writeln!(f, "  Mean error:          {:.3}", self.mean_centroid_error)?;
        writeln!(
            f,
            "  Median error:        {:.3}",
            self.median_centroid_error
        )?;
        writeln!(f, "  Max error:           {:.3}", self.max_centroid_error)?;
        writeln!(f, "  Std deviation:       {:.3}", self.std_centroid_error)?;
        writeln!(f)?;
        writeln!(f, "Property Accuracy:")?;
        writeln!(
            f,
            "  Mean FWHM error:     {:.1}%",
            self.mean_fwhm_error * 100.0
        )?;
        writeln!(
            f,
            "  Mean flux error:     {:.1}%",
            self.mean_flux_error * 100.0
        )?;
        Ok(())
    }
}

/// Compute detection metrics from ground truth and detected stars.
///
/// # Arguments
/// * `ground_truth` - True star positions and properties
/// * `detected` - Detected stars
/// * `match_radius` - Maximum distance for matching (typically 2 × FWHM)
pub fn compute_detection_metrics(
    ground_truth: &[GroundTruthStar],
    detected: &[Star],
    match_radius: f32,
) -> DetectionMetrics {
    let match_result = match_stars(ground_truth, detected, match_radius);

    let true_positives = match_result.matched_truth.len();
    let false_negatives = ground_truth.len() - true_positives;
    let false_positives = detected.len() - match_result.matched_detected.len();

    // Detection rate and precision
    let detection_rate = if ground_truth.is_empty() {
        1.0
    } else {
        true_positives as f32 / ground_truth.len() as f32
    };

    let precision = if detected.is_empty() {
        1.0
    } else {
        true_positives as f32 / detected.len() as f32
    };

    let f1_score = if detection_rate + precision > 0.0 {
        2.0 * detection_rate * precision / (detection_rate + precision)
    } else {
        0.0
    };

    let false_positive_rate = if detected.is_empty() {
        0.0
    } else {
        false_positives as f32 / detected.len() as f32
    };

    // Compute centroid errors
    let centroid_errors: Vec<f32> = match_result
        .pairs
        .iter()
        .map(|(_, _, dist)| *dist)
        .collect();

    let mean_centroid_error = if centroid_errors.is_empty() {
        0.0
    } else {
        centroid_errors.iter().sum::<f32>() / centroid_errors.len() as f32
    };

    let median_centroid_error = if centroid_errors.is_empty() {
        0.0
    } else {
        let mut sorted = centroid_errors.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        sorted[sorted.len() / 2]
    };

    let max_centroid_error = centroid_errors.iter().cloned().fold(0.0, f32::max);

    let std_centroid_error = if centroid_errors.len() > 1 {
        let variance = centroid_errors
            .iter()
            .map(|e| (e - mean_centroid_error).powi(2))
            .sum::<f32>()
            / centroid_errors.len() as f32;
        variance.sqrt()
    } else {
        0.0
    };

    // Compute FWHM and flux errors
    let mut fwhm_errors = Vec::new();
    let mut flux_errors = Vec::new();

    for &(ti, di, _) in &match_result.pairs {
        let truth = &ground_truth[ti];
        let det = &detected[di];

        // FWHM relative error
        if truth.fwhm > 0.0 {
            let fwhm_err = (det.fwhm - truth.fwhm).abs() / truth.fwhm;
            fwhm_errors.push(fwhm_err);
        }

        // Flux relative error
        if truth.flux > 0.0 {
            let flux_err = (det.flux - truth.flux).abs() / truth.flux;
            flux_errors.push(flux_err);
        }
    }

    let mean_fwhm_error = if fwhm_errors.is_empty() {
        0.0
    } else {
        fwhm_errors.iter().sum::<f32>() / fwhm_errors.len() as f32
    };

    let mean_flux_error = if flux_errors.is_empty() {
        0.0
    } else {
        flux_errors.iter().sum::<f32>() / flux_errors.len() as f32
    };

    DetectionMetrics {
        true_positives,
        false_positives,
        false_negatives,
        total_truth: ground_truth.len(),
        total_detected: detected.len(),
        detection_rate,
        precision,
        f1_score,
        false_positive_rate,
        centroid_errors,
        mean_centroid_error,
        median_centroid_error,
        max_centroid_error,
        std_centroid_error,
        fwhm_errors,
        mean_fwhm_error,
        flux_errors,
        mean_flux_error,
        match_result: Some(match_result),
    }
}

/// Save metrics to a text file.
pub fn save_metrics(metrics: &DetectionMetrics, path: &Path) {
    let mut file = File::create(path).expect("Failed to create metrics file");
    write!(file, "{}", metrics).expect("Failed to write metrics");
}

/// Pass/fail criteria for visual tests.
#[derive(Debug, Clone)]
pub struct PassCriteria {
    /// Minimum detection rate
    pub min_detection_rate: f32,
    /// Maximum false positive rate
    pub max_false_positive_rate: f32,
    /// Maximum mean centroid error (pixels)
    pub max_mean_centroid_error: f32,
    /// Maximum FWHM error (relative)
    pub max_fwhm_error: f32,
}

impl Default for PassCriteria {
    fn default() -> Self {
        Self {
            min_detection_rate: 0.95,
            max_false_positive_rate: 0.05,
            max_mean_centroid_error: 0.2,
            max_fwhm_error: 0.25,
        }
    }
}

/// Standard test criteria.
pub fn standard_criteria() -> PassCriteria {
    PassCriteria {
        min_detection_rate: 0.98,
        max_false_positive_rate: 0.02,
        max_mean_centroid_error: 0.1,
        max_fwhm_error: 0.15,
    }
}

/// Crowded field criteria (relaxed).
pub fn crowded_criteria() -> PassCriteria {
    PassCriteria {
        min_detection_rate: 0.90,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.2,
        max_fwhm_error: 0.25,
    }
}

/// Faint star criteria (relaxed).
pub fn faint_star_criteria() -> PassCriteria {
    PassCriteria {
        min_detection_rate: 0.80,
        max_false_positive_rate: 0.10,
        max_mean_centroid_error: 0.5,
        max_fwhm_error: 0.40,
    }
}

/// Check if metrics pass the given criteria.
pub fn check_pass(metrics: &DetectionMetrics, criteria: &PassCriteria) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();

    if metrics.detection_rate < criteria.min_detection_rate {
        failures.push(format!(
            "Detection rate {:.1}% < {:.1}%",
            metrics.detection_rate * 100.0,
            criteria.min_detection_rate * 100.0
        ));
    }

    if metrics.false_positive_rate > criteria.max_false_positive_rate {
        failures.push(format!(
            "False positive rate {:.1}% > {:.1}%",
            metrics.false_positive_rate * 100.0,
            criteria.max_false_positive_rate * 100.0
        ));
    }

    if metrics.mean_centroid_error > criteria.max_mean_centroid_error {
        failures.push(format!(
            "Mean centroid error {:.3}px > {:.3}px",
            metrics.mean_centroid_error, criteria.max_mean_centroid_error
        ));
    }

    if metrics.mean_fwhm_error > criteria.max_fwhm_error {
        failures.push(format!(
            "Mean FWHM error {:.1}% > {:.1}%",
            metrics.mean_fwhm_error * 100.0,
            criteria.max_fwhm_error * 100.0
        ));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_truth(x: f32, y: f32) -> GroundTruthStar {
        GroundTruthStar {
            x,
            y,
            flux: 100.0,
            fwhm: 3.0,
            eccentricity: 0.0,
            is_saturated: false,
            angle: 0.0,
        }
    }

    fn make_det(x: f32, y: f32) -> Star {
        Star {
            x,
            y,
            flux: 100.0,
            fwhm: 3.0,
            eccentricity: 0.0,
            snr: 50.0,
            peak: 0.5,
            sharpness: 0.3,
            roundness1: 0.0,
            roundness2: 0.0,
            laplacian_snr: 0.0,
        }
    }

    #[test]
    fn test_perfect_detection() {
        let truth = vec![make_truth(10.0, 10.0), make_truth(50.0, 50.0)];
        let detected = vec![make_det(10.0, 10.0), make_det(50.0, 50.0)];

        let metrics = compute_detection_metrics(&truth, &detected, 5.0);

        assert_eq!(metrics.true_positives, 2);
        assert_eq!(metrics.false_positives, 0);
        assert_eq!(metrics.false_negatives, 0);
        assert!((metrics.detection_rate - 1.0).abs() < 0.01);
        assert!((metrics.precision - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_with_false_positive() {
        let truth = vec![make_truth(10.0, 10.0)];
        let detected = vec![make_det(10.0, 10.0), make_det(100.0, 100.0)];

        let metrics = compute_detection_metrics(&truth, &detected, 5.0);

        assert_eq!(metrics.true_positives, 1);
        assert_eq!(metrics.false_positives, 1);
        assert_eq!(metrics.false_negatives, 0);
        assert!((metrics.detection_rate - 1.0).abs() < 0.01);
        assert!((metrics.precision - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_with_missed_star() {
        let truth = vec![make_truth(10.0, 10.0), make_truth(100.0, 100.0)];
        let detected = vec![make_det(10.0, 10.0)];

        let metrics = compute_detection_metrics(&truth, &detected, 5.0);

        assert_eq!(metrics.true_positives, 1);
        assert_eq!(metrics.false_positives, 0);
        assert_eq!(metrics.false_negatives, 1);
        assert!((metrics.detection_rate - 0.5).abs() < 0.01);
        assert!((metrics.precision - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_centroid_error() {
        let truth = vec![make_truth(10.0, 10.0)];
        let detected = vec![make_det(10.1, 10.2)]; // 0.224 pixel error

        let metrics = compute_detection_metrics(&truth, &detected, 5.0);

        assert!(metrics.mean_centroid_error > 0.2 && metrics.mean_centroid_error < 0.25);
    }

    #[test]
    fn test_pass_criteria() {
        let metrics = DetectionMetrics {
            detection_rate: 0.99,
            false_positive_rate: 0.01,
            mean_centroid_error: 0.05,
            mean_fwhm_error: 0.1,
            ..Default::default()
        };

        assert!(check_pass(&metrics, &standard_criteria()).is_ok());
    }

    #[test]
    fn test_fail_criteria() {
        let metrics = DetectionMetrics {
            detection_rate: 0.80, // Below standard
            false_positive_rate: 0.01,
            mean_centroid_error: 0.05,
            mean_fwhm_error: 0.1,
            ..Default::default()
        };

        let result = check_pass(&metrics, &standard_criteria());
        assert!(result.is_err());
    }
}
