//! Quantitative graders for synthetic-recovery tests.
//!
//! These turn "did the stage work?" into hand-checkable numbers — detection completeness
//! and reliability, astrometric error, rejection precision/recall, and noise statistics — so
//! tests assert bounds you can compute on paper rather than eyeballing.

use glam::DVec2;
use std::collections::HashSet;

/// Match recovered points to truth by nearest-neighbour within `max_dist`.
///
/// Greedy in ascending distance; each truth and each recovered point is used at most once.
/// Returns `(truth_idx, recovered_idx)` pairs.
pub fn match_catalogs(truth: &[DVec2], recovered: &[DVec2], max_dist: f64) -> Vec<(usize, usize)> {
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (ti, t) in truth.iter().enumerate() {
        for (ri, r) in recovered.iter().enumerate() {
            let d = t.distance(*r);
            if d <= max_dist {
                candidates.push((d, ti, ri));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("distances are finite"));

    let mut used_t = vec![false; truth.len()];
    let mut used_r = vec![false; recovered.len()];
    let mut pairs = Vec::new();
    for (_, ti, ri) in candidates {
        if !used_t[ti] && !used_r[ri] {
            used_t[ti] = true;
            used_r[ri] = true;
            pairs.push((ti, ri));
        }
    }
    pairs
}

/// Detection completeness & reliability derived from a catalog match.
#[derive(Debug, Clone, Copy)]
pub struct DetectionScore {
    pub matched: usize,
    pub n_truth: usize,
    pub n_recovered: usize,
}

impl DetectionScore {
    /// Fraction of true sources recovered (1.0 for an empty truth set).
    pub fn completeness(&self) -> f64 {
        if self.n_truth == 0 {
            1.0
        } else {
            self.matched as f64 / self.n_truth as f64
        }
    }

    /// Fraction of detections that are real — `1 - false_positive_rate` (1.0 for no detections).
    pub fn reliability(&self) -> f64 {
        if self.n_recovered == 0 {
            1.0
        } else {
            self.matched as f64 / self.n_recovered as f64
        }
    }
}

/// Score a detector's output catalog against truth within `max_dist` pixels.
pub fn score_detection(truth: &[DVec2], recovered: &[DVec2], max_dist: f64) -> DetectionScore {
    DetectionScore {
        matched: match_catalogs(truth, recovered, max_dist).len(),
        n_truth: truth.len(),
        n_recovered: recovered.len(),
    }
}

/// RMS positional error (pixels) over matched pairs; `INFINITY` if nothing matches.
pub fn astrometric_rms(truth: &[DVec2], recovered: &[DVec2], max_dist: f64) -> f64 {
    let pairs = match_catalogs(truth, recovered, max_dist);
    if pairs.is_empty() {
        return f64::INFINITY;
    }
    let sum_sq: f64 = pairs
        .iter()
        .map(|&(ti, ri)| truth[ti].distance_squared(recovered[ri]))
        .sum();
    (sum_sq / pairs.len() as f64).sqrt()
}

/// Precision/recall of a rejection mask against the truly-injected outlier set.
///
/// Both arguments are sets of opaque indices (e.g. a pixel index, or a `(frame, pixel)`
/// pair encoded as one `usize`).
#[derive(Debug, Clone, Copy)]
pub struct RejectionScore {
    pub precision: f64,
    pub recall: f64,
    pub true_positive: usize,
    pub false_positive: usize,
    pub false_negative: usize,
}

/// Grade a set of `rejected` indices against the `injected` outliers.
pub fn score_rejection(rejected: &[usize], injected: &[usize]) -> RejectionScore {
    let inj: HashSet<usize> = injected.iter().copied().collect();
    let rej: HashSet<usize> = rejected.iter().copied().collect();
    let tp = rej.iter().filter(|p| inj.contains(p)).count();
    let fp = rej.len() - tp;
    let fneg = inj.iter().filter(|p| !rej.contains(p)).count();
    RejectionScore {
        precision: if rej.is_empty() {
            1.0
        } else {
            tp as f64 / rej.len() as f64
        },
        recall: if inj.is_empty() {
            1.0
        } else {
            tp as f64 / inj.len() as f64
        },
        true_positive: tp,
        false_positive: fp,
        false_negative: fneg,
    }
}

/// Mean and (population) standard deviation of a pixel slice.
#[derive(Debug, Clone, Copy)]
pub struct PixelStats {
    pub mean: f64,
    pub std: f64,
}

/// Compute mean and standard deviation of `pixels`.
pub fn pixel_stats(pixels: &[f32]) -> PixelStats {
    let n = pixels.len();
    if n == 0 {
        return PixelStats {
            mean: 0.0,
            std: 0.0,
        };
    }
    let mean = pixels.iter().map(|&p| p as f64).sum::<f64>() / n as f64;
    let var = pixels
        .iter()
        .map(|&p| (p as f64 - mean).powi(2))
        .sum::<f64>()
        / n as f64;
    PixelStats {
        mean,
        std: var.sqrt(),
    }
}

/// Root-mean-square difference between two equal-length pixel slices.
pub fn rms_diff(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "slice lengths differ");
    if a.is_empty() {
        return 0.0;
    }
    let s: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
        .sum();
    (s / a.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::metrics::*;

    #[test]
    fn match_catalogs_greedy_nearest_within_radius() {
        let truth = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
        let recovered = [
            DVec2::new(0.1, 0.0),   // → truth 0
            DVec2::new(9.5, 0.0),   // → truth 1
            DVec2::new(50.0, 50.0), // spurious, beyond radius
        ];
        let pairs = match_catalogs(&truth, &recovered, 2.0);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(0, 0)));
        assert!(pairs.contains(&(1, 1)));
    }

    #[test]
    fn match_catalogs_respects_max_dist() {
        let truth = [DVec2::new(0.0, 0.0)];
        let recovered = [DVec2::new(3.0, 0.0)];
        assert!(match_catalogs(&truth, &recovered, 2.0).is_empty());
        assert_eq!(match_catalogs(&truth, &recovered, 4.0).len(), 1);
    }

    #[test]
    fn detection_completeness_and_reliability() {
        // 2 true, 3 detected (2 real + 1 spurious).
        let truth = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 10.0)];
        let recovered = [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(99.0, 99.0),
        ];
        let s = score_detection(&truth, &recovered, 1.0);
        assert_eq!(s.completeness(), 1.0);
        assert!((s.reliability() - 2.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn astrometric_rms_exact() {
        // distances 0.5 and 0 → rms = sqrt((0.25+0)/2) = 0.35355.
        let truth = [DVec2::new(0.0, 0.0), DVec2::new(20.0, 0.0)];
        let recovered = [DVec2::new(0.3, 0.4), DVec2::new(20.0, 0.0)];
        let rms = astrometric_rms(&truth, &recovered, 2.0);
        assert!((rms - (0.125f64).sqrt()).abs() < 1e-12, "rms {rms}");
    }

    #[test]
    fn rejection_precision_recall_exact() {
        // injected {1,2,3}, rejected {2,3,4}: tp=2, fp=1, fn=1.
        let s = score_rejection(&[2, 3, 4], &[1, 2, 3]);
        assert_eq!(
            (s.true_positive, s.false_positive, s.false_negative),
            (2, 1, 1)
        );
        assert!((s.precision - 2.0 / 3.0).abs() < 1e-12);
        assert!((s.recall - 2.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn pixel_stats_and_rms_diff_exact() {
        let s = pixel_stats(&[1.0, -1.0, 1.0, -1.0]);
        assert!(s.mean.abs() < 1e-12);
        assert!((s.std - 1.0).abs() < 1e-12);
        // diffs 0 and 2 → rms = sqrt((0+4)/2) = sqrt(2).
        assert!((rms_diff(&[1.0, 2.0], &[1.0, 4.0]) - 2.0f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn graders_handle_empty_and_degenerate_inputs() {
        let none: [DVec2; 0] = [];
        let some = [DVec2::new(1.0, 1.0)];
        // No truth → completeness 1.0; no detections → reliability 1.0 and completeness 0.
        assert_eq!(score_detection(&none, &some, 2.0).completeness(), 1.0);
        let s = score_detection(&some, &none, 2.0);
        assert_eq!(s.reliability(), 1.0);
        assert_eq!(s.completeness(), 0.0);
        // No in-radius match → infinite RMS.
        assert_eq!(
            astrometric_rms(&some, &[DVec2::new(99.0, 99.0)], 2.0),
            f64::INFINITY
        );
        // Empty rejection sets → precision and recall both 1.0.
        let r = score_rejection(&[], &[]);
        assert_eq!((r.precision, r.recall), (1.0, 1.0));
        // Empty pixel slices → zero stats.
        assert_eq!(pixel_stats(&[]).mean, 0.0);
        assert_eq!(rms_diff(&[], &[]), 0.0);
        // Two recovered near one truth → greedy nearest pairs only the closer one.
        let truth = [DVec2::new(0.0, 0.0)];
        let recovered = [DVec2::new(0.3, 0.0), DVec2::new(0.9, 0.0)];
        assert_eq!(match_catalogs(&truth, &recovered, 2.0), vec![(0, 0)]);
    }
}
