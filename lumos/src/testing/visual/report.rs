//! The text report a visual detection test writes beside its images.

use crate::stacking::star_detection::star::Star;
use crate::testing::synthetic::metrics::{DetectionScore, match_catalogs};
use crate::testing::synthetic::observe::ObservedSource;
use common::internals;
use glam::DVec2;
use std::fmt;
use std::fs::File;
use std::io::Write;

/// What a detection run recovered: the catalog match's [`DetectionScore`], and how far the
/// matched stars' measurements are from the truth.
#[derive(Debug, Clone)]
pub(crate) struct DetectionMetrics {
    score: DetectionScore,
    /// Distances from each matched star to its true position, in pixels.
    centroid: ErrorSummary,
    /// Mean relative FWHM error over the matched stars whose true FWHM is positive.
    mean_fwhm_error: f64,
    /// Mean relative flux error over the matched stars whose true flux is positive.
    mean_flux_error: f64,
}

/// Mean, median, largest and standard deviation of a set of errors; all zero for none.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ErrorSummary {
    mean: f64,
    median: f64,
    max: f64,
    std: f64,
}

impl ErrorSummary {
    /// The median is the upper middle value of an even count.
    fn of(mut errors: Vec<f64>) -> Self {
        if errors.is_empty() {
            return Self::default();
        }
        errors.sort_by(f64::total_cmp);
        let n = errors.len() as f64;
        let mean = errors.iter().sum::<f64>() / n;
        let variance = errors.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n;
        Self {
            mean,
            median: errors[errors.len() / 2],
            max: errors[errors.len() - 1],
            std: variance.sqrt(),
        }
    }
}

impl DetectionMetrics {
    /// Match `detected` to `ground_truth` within `match_radius` pixels and grade the matches.
    pub(crate) fn measure(
        ground_truth: &[ObservedSource],
        detected: &[Star],
        match_radius: f64,
    ) -> Self {
        let truth_positions: Vec<DVec2> = ground_truth.iter().map(|s| s.pos).collect();
        let detected_positions: Vec<DVec2> = detected.iter().map(|s| s.pos).collect();
        let pairs = match_catalogs(&truth_positions, &detected_positions, match_radius);
        let relative = |truth: f32, measured: f32| {
            (truth > 0.0).then(|| f64::from((measured - truth).abs() / truth))
        };
        let mean = |errors: Vec<f64>| ErrorSummary::of(errors).mean;
        Self {
            score: DetectionScore {
                matched: pairs.len(),
                n_truth: ground_truth.len(),
                n_recovered: detected.len(),
            },
            centroid: ErrorSummary::of(
                pairs
                    .iter()
                    .map(|&(ti, di)| truth_positions[ti].distance(detected_positions[di]))
                    .collect(),
            ),
            mean_fwhm_error: mean(
                pairs
                    .iter()
                    .filter_map(|&(ti, di)| relative(ground_truth[ti].fwhm, detected[di].fwhm))
                    .collect(),
            ),
            mean_flux_error: mean(
                pairs
                    .iter()
                    .filter_map(|&(ti, di)| relative(ground_truth[ti].flux, detected[di].flux))
                    .collect(),
            ),
        }
    }
}

impl fmt::Display for DetectionMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let score = &self.score;
        let (completeness, reliability) = (score.completeness(), score.reliability());
        let f1 = if completeness + reliability > 0.0 {
            2.0 * completeness * reliability / (completeness + reliability)
        } else {
            0.0
        };
        writeln!(f, "Detection Metrics")?;
        writeln!(f)?;
        writeln!(f, "Counts:")?;
        writeln!(f, "  Ground truth stars:  {}", score.n_truth)?;
        writeln!(f, "  Detected stars:      {}", score.n_recovered)?;
        writeln!(f, "  True positives:      {}", score.matched)?;
        writeln!(
            f,
            "  False positives:     {}",
            score.n_recovered - score.matched
        )?;
        writeln!(
            f,
            "  False negatives:     {}",
            score.n_truth - score.matched
        )?;
        writeln!(f)?;
        writeln!(f, "Rates:")?;
        writeln!(f, "  Completeness:        {:.1}%", completeness * 100.0)?;
        writeln!(f, "  Reliability:         {:.1}%", reliability * 100.0)?;
        writeln!(f, "  F1 score:            {f1:.3}")?;
        writeln!(f)?;
        writeln!(f, "Centroid Accuracy (pixels):")?;
        writeln!(f, "  Mean error:          {:.3}", self.centroid.mean)?;
        writeln!(f, "  Median error:        {:.3}", self.centroid.median)?;
        writeln!(f, "  Max error:           {:.3}", self.centroid.max)?;
        writeln!(f, "  Std deviation:       {:.3}", self.centroid.std)?;
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
        )
    }
}

/// Write `metrics` as text to the debug file `name`, when debug output is on.
pub(crate) fn save_metrics(metrics: &DetectionMetrics, name: &str) {
    if let Some(path) = internals::debug_output_path(name) {
        let mut file = File::create(path).expect("Failed to create metrics file");
        write!(file, "{metrics}").expect("Failed to write metrics");
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::assertions::is_close;
    use crate::testing::visual::report::*;

    fn truth(x: f64, y: f64) -> ObservedSource {
        ObservedSource {
            pos: DVec2::new(x, y),
            flux: 100.0,
            fwhm: 3.0,
        }
    }

    fn star(x: f64, y: f64) -> Star {
        Star::at(DVec2::new(x, y)).with_eccentricity(0.0)
    }

    /// Matches, false positives and misses: two of two found, one found plus one spurious, and one
    /// of two found.
    #[test]
    fn counts_follow_the_catalog_match() {
        for (truth_stars, detected, matched) in [
            (
                vec![truth(10.0, 10.0), truth(50.0, 50.0)],
                vec![star(10.0, 10.0), star(50.0, 50.0)],
                2,
            ),
            (
                vec![truth(10.0, 10.0)],
                vec![star(10.0, 10.0), star(100.0, 100.0)],
                1,
            ),
            (
                vec![truth(10.0, 10.0), truth(100.0, 100.0)],
                vec![star(10.0, 10.0)],
                1,
            ),
        ] {
            let metrics = DetectionMetrics::measure(&truth_stars, &detected, 5.0);
            assert_eq!(
                (
                    metrics.score.matched,
                    metrics.score.n_truth,
                    metrics.score.n_recovered
                ),
                (matched, truth_stars.len(), detected.len())
            );
        }
    }

    /// Three stars off by 0.5, 1 and 2 px along x: mean 3.5/3, median 1, max 2, and variance
    /// ((2/3)² + (1/6)² + (5/6)²)/3 = 7/18, which a few f64 roundings reach within 4ε.
    #[test]
    fn centroid_errors_summarise_the_matched_offsets() {
        let truth_stars = [truth(10.0, 10.0), truth(50.0, 50.0), truth(90.0, 90.0)];
        let detected = [star(10.5, 10.0), star(51.0, 50.0), star(92.0, 90.0)];
        let centroid = DetectionMetrics::measure(&truth_stars, &detected, 5.0).centroid;
        assert_eq!(
            (centroid.mean, centroid.median, centroid.max),
            (3.5 / 3.0, 1.0, 2.0)
        );
        assert!(is_close(
            centroid.std,
            (7.0f64 / 18.0).sqrt(),
            4.0 * f64::EPSILON
        ));
        assert_eq!(ErrorSummary::of(Vec::new()), ErrorSummary::default());
    }
}
