use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::combine::rejection::winsorized_clip_config::WinsorizedEstimate;
use crate::combine::rejection::*;
use crate::internals::prelude::*;
use crate::math::statistics::mad_fast;
use crate::math::sum::mean_f32;
use std::f64::consts::TAU;

fn scratch() -> ScratchBuffers {
    ScratchBuffers::default()
}

/// The frames `reject` keeps of `values`, ascending, after checking that every survivor's value
/// still sits beside its own frame's index — a reorder that moved values without their indices
/// would pair the weights with the wrong frames.
fn survivors(
    values: &[f32],
    reject: impl Fn(&mut [f32], &mut ScratchBuffers) -> usize,
) -> Vec<usize> {
    let mut working = values.to_vec();
    let mut scratch = scratch();
    let remaining = reject(&mut working, &mut scratch);
    for (value, &index) in working[..remaining].iter().zip(&scratch.indices) {
        assert_eq!(*value, values[index], "a survivor lost its frame index");
    }
    let mut kept = scratch.indices[..remaining].to_vec();
    kept.sort_unstable();
    kept
}

/// Every frame of `frames` but those in `dropped`.
fn all_but(frames: usize, dropped: &[usize]) -> Vec<usize> {
    (0..frames).filter(|i| !dropped.contains(i)).collect()
}

/// Every config's documented defaults, its constructors, and the `Rejection` shorthands.
#[test]
fn rejection_configs_default_and_construct_as_documented() {
    let symmetric = |sigma| SigmaBounds::symmetric(sigma);
    assert_eq!(
        SigmaClipConfig::default(),
        SigmaClipConfig {
            sigma: symmetric(2.5),
            max_iterations: 3
        }
    );
    assert_eq!(
        WinsorizedClipConfig::default(),
        WinsorizedClipConfig {
            sigma: symmetric(2.5)
        }
    );
    assert_eq!(
        LinearFitClipConfig::default(),
        LinearFitClipConfig {
            sigma: symmetric(3.0),
            max_iterations: 3
        }
    );
    assert_eq!(
        PercentileClipConfig::default(),
        PercentileClipConfig {
            low_percentile: 10.0,
            high_percentile: 10.0
        }
    );
    assert_eq!(
        GesdConfig::default(),
        GesdConfig {
            alpha: 0.05,
            max_outliers: None
        }
    );
    assert_eq!(
        SigmaClipConfig::new_asymmetric(2.0, 3.0, 5),
        SigmaClipConfig {
            sigma: SigmaBounds::asymmetric(2.0, 3.0),
            max_iterations: 5
        }
    );

    for (shorthand, expected) in [
        (
            Rejection::default(),
            Rejection::SigmaClip(SigmaClipConfig::default()),
        ),
        (
            Rejection::sigma_clip(2.0),
            Rejection::SigmaClip(SigmaClipConfig::new_asymmetric(2.0, 2.0, 3)),
        ),
        (
            Rejection::sigma_clip_asymmetric(4.0, 2.0),
            Rejection::SigmaClip(SigmaClipConfig::new_asymmetric(4.0, 2.0, 3)),
        ),
        (
            Rejection::winsorized(3.0),
            Rejection::Winsorized(WinsorizedClipConfig::new_asymmetric(3.0, 3.0)),
        ),
        (
            Rejection::linear_fit(2.5),
            Rejection::LinearFit(LinearFitClipConfig::new(2.5, 2.5, 3)),
        ),
        (
            Rejection::percentile(15.0),
            Rejection::Percentile(PercentileClipConfig::new(15.0, 15.0)),
        ),
        (Rejection::gesd(), Rejection::Gesd(GesdConfig::default())),
    ] {
        assert_eq!(shorthand, expected);
    }
}

#[test]
fn gesd_config_default() {
    let config = GesdConfig::default();
    let automatic_cases = [
        (0, 0),
        (3, 0),
        (4, 1),
        (8, 2),
        (15, 2),
        (24, 2),
        (25, 6),
        (39, 9),
        (40, 10),
        (44, 10),
        (100, 10),
    ];
    for (sample_count, expected) in automatic_cases {
        assert_eq!(
            config.max_outliers_for_size(sample_count),
            expected,
            "automatic limit for {sample_count} samples"
        );
    }

    for (sample_count, configured) in [(15, 3), (44, 11), (100, 25)] {
        assert_eq!(
            GesdConfig::new(0.05, Some(configured)).max_outliers_for_size(sample_count),
            configured,
            "explicit limit for {sample_count} samples"
        );
    }
}

/// Which frames sigma clipping keeps, by hand. σ is 1.4826 × the MAD about the median.
///
/// - `ramp`, [1, 1.5, …, 4, 100]: median 2.75, deviations ranked .25 .25 .75 .75 1.25 1.25 1.75
///   97.25, MAD 1 → σ 1.4826, so 2σ keeps up to 5.72 and drops the 100. The seven left: median
///   2.5, MAD 1 again, nothing more. The same at 2.5σ, and with a high threshold of 2 under a low
///   one of 4.
/// - `low_kept`, [−5, 1, 1.5, …, 4.5, 50] at low 10σ, high 2σ: median 2.75, MAD 1.25 → σ 1.853;
///   the high cut 6.46 drops the 50, the low cut −15.8 keeps the −5. Then median 2.5, MAD 1:
///   both cuts keep everything left.
/// - `three_high`, [1, 1.5, 2, 2.5, 3, 50, 80, 100]: median 2.75, MAD 1.5 → σ 2.224, 2σ keeps up
///   to 7.2: the three high values go. The five left: median 2, MAD 0.5 → σ 0.741, 2σ keeps
///   [0.52, 3.48], all of them.
/// - `clean`, [1, 1.125, 1.25, 0.875, 1.0625] at 3σ: median 1.0625, MAD 0.0625 → 3σ 0.278 keeps
///   all five.
/// - `bright`, fourteen values 7990–8010 about 8000 and a 9000: the f64 screen sees the 9000 past
///   2.5 trimmed standard deviations, and the clip drops it.
/// - `two_levels`, 47 × 9, 47 × 11 and six values from 100 to 800: median 11, MAD 0 for the 11s
///   and 2 for the 9s — rank 50 of the deviations is a 2 — so σ 2.97 keeps [3.6, 18.4].
#[test]
fn sigma_clip_keeps_exactly_the_frames_its_bands_hold() {
    let ramp = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let low_kept = [-5.0, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 50.0];
    let three_high = [1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0];
    let clean = [1.0, 1.125, 1.25, 0.875, 1.0625];
    let bright = [
        7990.0, 8000.0, 8010.0, 7995.0, 8005.0, 8000.0, 7990.0, 8010.0, 8000.0, 7995.0, 8005.0,
        8000.0, 7990.0, 8010.0, 9000.0,
    ];
    let two_levels: Vec<f32> = [9.0; 47]
        .into_iter()
        .chain([11.0; 47])
        .chain([100.0, 200.0, 500.0, 600.0, 700.0, 800.0])
        .collect();
    for (name, values, config, kept) in [
        (
            "ramp 2σ",
            &ramp[..],
            SigmaClipConfig::new(2.0, 3),
            all_but(8, &[7]),
        ),
        (
            "ramp 2.5σ",
            &ramp,
            SigmaClipConfig::new(2.5, 3),
            all_but(8, &[7]),
        ),
        (
            "ramp 4σ low, 2σ high",
            &ramp,
            SigmaClipConfig::new_asymmetric(4.0, 2.0, 3),
            all_but(8, &[7]),
        ),
        (
            "low kept",
            &low_kept,
            SigmaClipConfig::new_asymmetric(10.0, 2.0, 5),
            all_but(10, &[9]),
        ),
        (
            "three high",
            &three_high,
            SigmaClipConfig::new(2.0, 3),
            all_but(8, &[5, 6, 7]),
        ),
        (
            "clean",
            &clean,
            SigmaClipConfig::new(3.0, 3),
            all_but(5, &[]),
        ),
        (
            "bright",
            &bright,
            SigmaClipConfig::new(2.5, 3),
            all_but(15, &[14]),
        ),
        (
            "two levels",
            &two_levels,
            SigmaClipConfig::new(2.5, 3),
            all_but(94, &[]),
        ),
        (
            "two samples",
            &[1.0, 2.0],
            SigmaClipConfig::default(),
            all_but(2, &[]),
        ),
    ] {
        assert_eq!(
            survivors(values, |values, scratch| config.reject(values, scratch)),
            kept,
            "{name}"
        );
    }
}

/// The asymmetric form at equal thresholds is the symmetric one: same survivors, same values.
#[test]
fn sigma_clip_symmetric_equals_asymmetric_same_thresholds() {
    let values = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let (mut symmetric, mut asymmetric) = (values, values);
    let (mut first, mut second) = (scratch(), scratch());
    let kept = SigmaClipConfig::new(2.5, 3).reject(&mut symmetric, &mut first);
    let also_kept =
        SigmaClipConfig::new_asymmetric(2.5, 2.5, 3).reject(&mut asymmetric, &mut second);
    assert_eq!(kept, also_kept);
    assert_eq!(symmetric[..kept], asymmetric[..kept]);
    assert_eq!(first.indices[..kept], second.indices[..kept]);
}

/// `sorted_mad` is the upper-middle order statistic of the absolute deviations, as `mad_fast`
/// is: odd and even lengths, a centre inside and outside the data, duplicates, a heavy outlier.
#[test]
fn sorted_mad_matches_mad_fast() {
    let cases: &[&[f32]] = &[
        &[1.0, 2.0, 3.0, 4.0, 100.0],
        &[1.0, 2.0, 3.0, 4.0],
        &[-5.0, 0.0, 0.0, 0.0, 1.0, 2.0],
        &[10.0, 10.0, 10.0],
        &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
    ];
    let mut buf = vec![];
    for sorted in cases {
        let mid = sorted[sorted.len() / 2];
        for &center in &[mid, sorted[0], mid + 0.05] {
            assert_eq!(
                sorted_mad(sorted, center),
                mad_fast(sorted, center, &mut buf),
                "sorted_mad({sorted:?}, {center})"
            );
        }
    }
}

/// The rejection centre and its MAD are the median of an even count, not its upper-middle element.
/// [1, 3, 7, 9]: centre (3 + 7)/2 = 5; deviations [4, 2, 2, 4] ranked [2, 2, 4, 4] → (2 + 4)/2 = 3.
/// [1, 3, 7]: centre 3; deviations [2, 0, 4] ranked [0, 2, 4] → 2.
#[test]
fn rejection_centre_and_mad_are_medians_at_both_parities() {
    assert_eq!(sorted_median(&[1.0, 3.0, 7.0, 9.0]), 5.0);
    assert_eq!(sorted_mad(&[1.0, 3.0, 7.0, 9.0], 5.0), 3.0);
    assert_eq!(sorted_median(&[1.0, 3.0, 7.0]), 3.0);
    assert_eq!(sorted_mad(&[1.0, 3.0, 7.0], 3.0), 2.0);
}

/// Which frames linear-fit clipping keeps, by hand. Its first pass is a median ± k·σ clip; each
/// pass after it fits a line through the sorted survivors and rejects by residual, σ being the
/// mean absolute residual.
///
/// - `off_line`, [1, 2, 3, 4, 100, 6] at 2σ: median 3.5, MAD 2 → σ 2.97, so the seed pass drops
///   the 100. The line through [1, 2, 3, 4, 6] is `3.2 + 1.2·(x − 2)`, with residuals 0.2, 0,
///   −0.2, −0.4, 0.4, all within 2 × their mean 0.24 — none goes.
/// - `hidden`, ramp 10 … 90 and a 5 at 2σ: the seed (median 45, σ 37) keeps all ten; the line
///   through the sorted ten has a mean |residual| of 0.92, and only the 5, 3.3 off it, passes 1.84.
/// - `top`, [1 … 7, 100]: the seed (median 4.5, MAD 2 → σ 2.97) drops the 100, and [1 … 7] is a
///   line.
/// - `middle`, the same with the 100 a 50 among them: the seed drops it the same way.
/// - `one_pass`, [10, 10.5, 11, 10.2, 10.8, 10.3, 10.7, 50] with one pass at 3σ: the seed alone,
///   median 10.6, MAD 0.35 → σ 0.519, drops the 50.
/// - `constant` has no spread, `trend` [1, 3, …, 15] is a line, and two frames are too few to fit:
///   nothing goes.
/// - `long`, 100 frames on the line `y = x` with frame 50 at 1000: the seed (median 50, σ 37)
///   drops it, and the rest, one step off a line at the gap, stays within 3σ of its fit.
#[test]
fn linear_fit_keeps_exactly_the_frames_on_its_line() {
    let mut long: Vec<f32> = (0..100).map(|i| i as f32).collect();
    long[50] = 1000.0;
    for (name, values, config, kept) in [
        (
            "off line",
            &[1.0, 2.0, 3.0, 4.0, 100.0, 6.0][..],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(6, &[4]),
        ),
        (
            "hidden",
            &[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 5.0],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(10, &[9]),
        ),
        (
            "top",
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(8, &[7]),
        ),
        (
            "middle",
            &[1.0, 2.0, 3.0, 50.0, 5.0, 6.0, 7.0, 4.0],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(8, &[3]),
        ),
        (
            "one pass",
            &[10.0, 10.5, 11.0, 10.2, 10.8, 10.3, 10.7, 50.0],
            LinearFitClipConfig::new(3.0, 3.0, 1),
            all_but(8, &[7]),
        ),
        (
            "constant",
            &[5.0; 5],
            LinearFitClipConfig::default(),
            all_but(5, &[]),
        ),
        (
            "trend",
            &[1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(8, &[]),
        ),
        (
            "two samples",
            &[1.0, 2.0],
            LinearFitClipConfig::default(),
            all_but(2, &[]),
        ),
        (
            "long",
            &long,
            LinearFitClipConfig::new(3.0, 3.0, 3),
            all_but(100, &[50]),
        ),
    ] {
        assert_eq!(
            survivors(values, |values, scratch| config.reject(values, scratch)),
            kept,
            "{name}"
        );
    }
}

/// A long stack keeps its line: 4001 frames on the ramp `1000 + i/4` and one 3 above where the
/// ramp would continue. The seed pass keeps it (median 1500, σ ≈ 741, 3σ far past it). The fit's
/// σ is the mean |residual|, about 3/4002 from the outlier plus the tilt it gives the line —
/// `(x − x̄)·3/Sxx` with `Sxx = n(n² − 1)/12 ≈ 5.3e9`, under 1.2e-6 per position, 2.3e-3 at the
/// ends — so 3σ is a few thousandths: every ramp frame stays, and the outlier, 3 off, goes.
#[test]
fn linear_fit_keeps_a_long_ramp_and_drops_its_outlier() {
    let mut values: Vec<f32> = (0..4001).map(|i| 1000.0 + i as f32 / 4.0).collect();
    values.push(1000.0 + 4001.0 / 4.0 + 3.0);
    assert_eq!(
        survivors(&values, |values, scratch| {
            LinearFitClipConfig::new(3.0, 3.0, 3).reject(values, scratch)
        }),
        all_but(4002, &[4001])
    );
}

/// On [1, 3, 5, 7, 50, 11, 13, 15] at 2σ the fit rejects a frame the median cannot see.
///
/// Both first passes are the same median clip: median 9, deviations ranked 2 2 4 4 6 6 8 41, MAD
/// 5 → σ 7.41, which drops the 50 alone. Sigma clipping then re-centres on [1, 3, 5, 7, 11, 13,
/// 15] — median 7, MAD 4 → 2σ = 11.9 — and keeps all seven. The line through them is
/// `55/7 + (68/28)·(x − 3)`, with residuals 0.43, 0, −0.43, −0.86, 0.71, 0.29, −0.14 and a mean
/// |residual| of 0.41, so 2σ = 0.82 rejects the 7.
#[test]
fn linear_fit_rejects_what_sigma_clip_keeps() {
    let values = [1.0, 3.0, 5.0, 7.0, 50.0, 11.0, 13.0, 15.0];
    assert_eq!(
        survivors(&values, |values, scratch| {
            SigmaClipConfig::new(2.0, 3).reject(values, scratch)
        }),
        all_but(8, &[4])
    );
    assert_eq!(
        survivors(&values, |values, scratch| {
            LinearFitClipConfig::new(2.0, 2.0, 3).reject(values, scratch)
        }),
        all_but(8, &[3, 4])
    );
    // One pass of each is the same median clip.
    assert_eq!(
        survivors(&values, |values, scratch| {
            SigmaClipConfig::new(2.0, 1).reject(values, scratch)
        }),
        all_but(8, &[4])
    );
    assert_eq!(
        survivors(&values, |values, scratch| {
            LinearFitClipConfig::new(2.0, 2.0, 1).reject(values, scratch)
        }),
        all_but(8, &[4])
    );
}

/// Which frames winsorized clipping keeps, by hand. Its centre and σ come from the Huber
/// estimate (`robust_estimate`), and the clip is k·σ about that centre.
///
/// - `ramp`, [1, 1.5, …, 4, 100] at 2σ: the estimate settles at centre 2.75, σ 1.53, so the band
///   [−0.3, 5.8] drops the 100.
/// - `high`, [1, 1.1, 1.2, 0.9, 1, 50] at low 3σ, high 2σ: centre 1.05, σ 0.18; the 50 goes.
/// - `clean`, [2, 2.125, 2.25, 1.875, 2.0625] at 3σ: centre 2.0625, σ = √(0.078125/4) × 1.134 =
///   0.158, and the widest deviation 0.1875 is inside 3σ: nothing goes.
/// - `mild`, [1, 1.1, 1.2, 0.9, 1, 1.1, 0.8, 1.3, 2]: centre 1.1, σ 0.224 (the fixed point
///   `winsorized_converges_to_its_huber_fixed_point` derives), so the 2.0 is 4.0σ out — kept at
///   10σ, gone at 2σ, where the next widest, 0.3 off, is 1.3σ.
#[test]
fn winsorized_keeps_exactly_the_frames_its_band_holds() {
    let mild = [1.0, 1.1, 1.2, 0.9, 1.0, 1.1, 0.8, 1.3, 2.0];
    for (name, values, config, kept) in [
        (
            "ramp",
            &[1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0][..],
            WinsorizedClipConfig::new(2.0),
            all_but(8, &[7]),
        ),
        (
            "high",
            &[1.0, 1.1, 1.2, 0.9, 1.0, 50.0],
            WinsorizedClipConfig::new_asymmetric(3.0, 2.0),
            all_but(6, &[5]),
        ),
        (
            "clean",
            &[2.0, 2.125, 2.25, 1.875, 2.0625],
            WinsorizedClipConfig::new(3.0),
            all_but(5, &[]),
        ),
        (
            "mild at 10σ",
            &mild,
            WinsorizedClipConfig::new(10.0),
            all_but(9, &[]),
        ),
        (
            "mild at 2σ",
            &mild,
            WinsorizedClipConfig::new(2.0),
            all_but(9, &[8]),
        ),
        (
            "two samples",
            &[1.0, 2.0],
            WinsorizedClipConfig::default(),
            all_but(2, &[]),
        ),
    ] {
        assert_eq!(
            survivors(values, |values, scratch| config.reject(values, scratch)),
            kept,
            "{name}"
        );
    }
}

/// The spread is the bias-corrected standard deviation about the median, not the MAD. Twenty
/// values 10 + i/8 have median 11.1875 and deviations ±1/16, ±3/16, …, ±19/16, whose squares sum
/// to 2·1330/256 = 10.390625: over 19 that is 0.546875, so σ = √0.546875 × 1.134 = 0.83860. No
/// value is past 1.5σ = 1.26, so nothing is clamped and the first estimate stands. The MAD would
/// give 1.4826 × 10/16 = 0.927 instead, and dropping the 1.134 would give 0.7395. Every step up to
/// the square root is exact in f32; the root, the product and 1.134's own rounding each cost at
/// most half an ulp near 0.84, 6e-8.
///
/// On 1 … 10 the same holds about 5.5: (2·(0.5² + 1.5² + … + 4.5²))/9 = 82.5/9 and σ = 3.43336,
/// within the same three roundings near 3.4, 2.4e-7 each.
#[test]
fn winsorized_sigma_is_the_corrected_standard_deviation() {
    let eighths: Vec<f32> = (0..20).map(|i| 10.0 + i as f32 / 8.0).collect();
    let WinsorizedEstimate { center, sigma } =
        WinsorizedClipConfig::robust_estimate(&eighths, &mut vec![]);
    assert_eq!(center, 11.1875);
    assert_close!(sigma, 0.546_875f64.sqrt() * 1.134, 1.8e-7);

    let ten: Vec<f32> = (1..=10).map(|i| i as f32).collect();
    let WinsorizedEstimate { center, sigma } =
        WinsorizedClipConfig::robust_estimate(&ten, &mut vec![]);
    assert_eq!(center, 5.5);
    assert_close!(sigma, (82.5f64 / 9.0).sqrt() * 1.134, 7.2e-7);
}

/// With an outlier, the clamp iterates to Huber's fixed point. On [10, 10.1, 10.2, 9.9, 10,
/// 10.1, 9.8, 10.3, 50] the median 10.1 stays put, the eight cluster values sit inside 1.5σ with
/// squared deviations summing to S = 0.2, and the 50 clamps to 10.1 + 1.5σ. So the fixed point
/// solves σ² = 1.134²·(S + 2.25σ²)/8: σ² = 0.0321489/0.638322, σ = 0.224421. The iteration stops
/// once a step moves σ by at most 0.05%; it contracts by r = 1.134²·2.25/8 = 0.3617, so it stops
/// within 0.0005σ·r/(1 − r) = 6.4e-5 of the fixed point.
#[test]
fn winsorized_converges_to_its_huber_fixed_point() {
    let values = [10.0, 10.1, 10.2, 9.9, 10.0, 10.1, 9.8, 10.3, 50.0];
    let WinsorizedEstimate { center, sigma } =
        WinsorizedClipConfig::robust_estimate(&values, &mut vec![]);
    assert_eq!(center, 10.1);
    let fixed_point = (1.134f64.powi(2) * 0.2 / 8.0 / (1.0 - 1.134f64.powi(2) * 2.25 / 8.0)).sqrt();
    assert_close!(sigma, fixed_point, 6.4e-5);
}

/// Percentile clipping drops `floor(p·n)` from each end of the sorted values. [5, 1, 3, 2, 4] at
/// 20% drops one each side, keeping 2, 3, 4 — frames 3, 2, 4 in that order. 100 values in reverse
/// order at 10% keep 10 … 89, which came from frames 89 down to 10. Two frames are too few.
#[test]
fn percentile_keeps_the_middle_of_the_sorted_frames() {
    let mut values = [5.0, 1.0, 3.0, 2.0, 4.0];
    let mut s = scratch();
    let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut s);
    assert_eq!(
        (&values[..remaining], &s.indices[..remaining]),
        (&[2.0, 3.0, 4.0][..], &[3, 2, 4][..])
    );

    let mut values: Vec<f32> = (0..100).rev().map(|i| i as f32).collect();
    let remaining = PercentileClipConfig::new(10.0, 10.0).reject(&mut values, &mut s);
    let expected_values: Vec<f32> = (10..90).map(|i| i as f32).collect();
    let expected_frames: Vec<usize> = (10..90).map(|v| 99 - v).collect();
    assert_eq!(values[..remaining], expected_values);
    assert_eq!(s.indices[..remaining], expected_frames);

    assert_eq!(
        survivors(&[1.0, 2.0], |values, scratch| {
            PercentileClipConfig::default().reject(values, scratch)
        }),
        [0, 1]
    );
}

/// The kept range: `floor(p·n)` off each end, or the middle element alone when the two meet. At
/// 49% of 5, two come off each end, leaving 2..3; a lone element survives any percentile.
#[test]
fn surviving_range_takes_whole_frames_off_each_end() {
    for (low, high, n, expected) in [
        (20.0, 20.0, 10, 2..8),
        (49.0, 49.0, 5, 2..3),
        (10.0, 10.0, 1, 0..1),
        (40.0, 40.0, 4, 1..3),
        (50.0, 50.0, 4, 2..3),
    ] {
        assert_eq!(
            PercentileClipConfig::new(low, high).surviving_range(n),
            expected,
            "{low}% / {high}% of {n}"
        );
    }
}

#[test]
fn gesd_removes_single_bright_outlier() {
    let values = [1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
    assert_eq!(
        survivors(&values, |values, scratch| {
            GesdConfig::new(0.05, None).reject(values, scratch)
        }),
        all_but(8, &[7])
    );
}

/// A constant set has no spread to test against, a tight one no value past the critical
/// statistic, and two frames are too few: nothing goes.
#[test]
fn gesd_keeps_sets_without_an_outlier() {
    for (name, values, config) in [
        ("constant", &[1.0; 8][..], GesdConfig::new(0.05, Some(3))),
        (
            "tight",
            &[1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1],
            GesdConfig::default(),
        ),
        ("two samples", &[1.0, 2.0], GesdConfig::default()),
    ] {
        assert_eq!(
            survivors(values, |values, scratch| config.reject(values, scratch)),
            all_but(values.len(), &[]),
            "{name}"
        );
    }
}

/// At an α no t quantile resolves, the critical value is the finite limit `(n − 1)/√n`.
#[test]
fn gesd_tiny_alpha_uses_finite_limiting_critical_value() {
    let mut values: Vec<f32> = (0..15).map(|value| value as f32).collect();
    let mut scratch = scratch();

    let remaining = GesdConfig::new(f32::MIN_POSITIVE, Some(3)).reject(&mut values, &mut scratch);

    assert_eq!(remaining, 15);
    assert_eq!(scratch.gesd.critical_values[0], 14.0 / 15.0f64.sqrt());
}

/// NIST's worked example (Engineering Statistics Handbook §1.3.5.17.3): 54 values, ten candidates
/// at α = 0.05, three outliers. The handbook prints each statistic and critical value cut to three
/// decimals (its 3.118 is 3.11891), so each is the computed one less under 0.001.
#[test]
fn gesd_matches_nist_reference_example() {
    let mut values = vec![
        -0.25, 0.68, 0.94, 1.15, 1.20, 1.26, 1.26, 1.34, 1.38, 1.43, 1.49, 1.49, 1.55, 1.56, 1.58,
        1.65, 1.69, 1.70, 1.76, 1.77, 1.81, 1.91, 1.94, 1.96, 1.99, 2.06, 2.09, 2.10, 2.14, 2.15,
        2.23, 2.24, 2.26, 2.35, 2.37, 2.40, 2.47, 2.54, 2.62, 2.64, 2.90, 2.92, 2.92, 2.93, 3.21,
        3.26, 3.30, 3.59, 3.68, 4.30, 4.64, 5.34, 5.42, 6.01,
    ];
    let expected = [
        (3.118, 3.158),
        (2.942, 3.151),
        (3.179, 3.143),
        (2.810, 3.136),
        (2.815, 3.128),
        (2.848, 3.120),
        (2.279, 3.111),
        (2.310, 3.103),
        (2.101, 3.094),
        (2.067, 3.085),
    ];
    let mut scratch = scratch();

    let remaining = GesdConfig::new(0.05, Some(10)).reject(&mut values, &mut scratch);

    assert_eq!(remaining, 51);
    assert!(scratch.indices[..remaining].iter().all(|&index| index < 51));
    for ((statistic, critical), (expected_statistic, expected_critical)) in scratch
        .gesd
        .statistics
        .iter()
        .zip(&scratch.gesd.critical_values)
        .zip(expected)
    {
        for (computed, printed) in [
            (*statistic, expected_statistic),
            (*critical, expected_critical),
        ] {
            assert!(
                (0.0..0.001).contains(&(computed - printed)),
                "{computed} does not cut to {printed}"
            );
        }
    }
}

#[test]
fn gesd_is_sign_symmetric_for_asymmetric_outliers() {
    let values = [
        -1.4, -1.2, -1.0, -0.8, -0.6, -0.4, -0.2, 0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2, 1.4, -8.0,
        10.0,
    ];
    let mirrored = values.map(|value: f32| -value);
    let config = GesdConfig::new(0.05, Some(2));
    let reject = |values: &mut [f32], scratch: &mut ScratchBuffers| config.reject(values, scratch);
    assert_eq!(survivors(&values, reject), all_but(17, &[15, 16]));
    assert_eq!(survivors(&mirrored, reject), all_but(17, &[15, 16]));
}

#[test]
fn gesd_gaussian_false_positive_rate_matches_alpha() {
    const ALPHA: f32 = 0.05;
    const TRIALS: usize = 4_000;

    // Not `TestRng`: it is an LCG, and Box-Muller over consecutive LCG outputs lays the pairs
    // on a handful of spirals rather than filling the plane. That distorts the tails, which is
    // exactly what a GESD outlier test measures — swapping this generator in moves the observed
    // false-positive rate from 0.050 to 0.076, past the 5-sigma bound below.
    let mut rng = ChaCha8Rng::seed_from_u64(0x947e_4d3a_7c16_b205);
    for sample_count in [15, 25, 50, 100] {
        let config = GesdConfig::new(ALPHA, Some(sample_count / 4));
        let mut scratch = scratch();
        let mut false_positives = 0usize;

        for _ in 0..TRIALS {
            let mut values: Vec<f32> = (0..sample_count)
                .map(|_| standard_normal(&mut rng))
                .collect();
            if config.reject(&mut values, &mut scratch) < sample_count {
                false_positives += 1;
            }
        }

        let actual = false_positives as f64 / TRIALS as f64;
        let expected = f64::from(ALPHA);
        let standard_error = (expected * (1.0 - expected) / TRIALS as f64).sqrt();
        assert!(
            (actual - expected).abs() <= 5.0 * standard_error,
            "n={sample_count}: expected false-positive rate {expected}, got {actual}"
        );
    }
}

fn standard_normal(rng: &mut ChaCha8Rng) -> f32 {
    let u1 = rng.random::<f64>().max(f64::MIN_POSITIVE);
    let u2 = rng.random::<f64>();
    ((-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()) as f32
}

/// `combine_mean` averages the survivors under their own frames' weights. Each expected value is
/// the weighted mean of the survivors the tests above derive, an exact quotient rounded once.
///
/// - No rejection over 1 … 5: 15/5, and with frame 0 weighing 10: (10 + 14)/14.
/// - Sigma clipping at 2σ drops the 100 of the ramp: 17.5/7. At low 4σ, high 2σ with frame 0
///   weighing 10: (10 + 16.5)/16.
/// - Percentile at 20% of 1 … 10 keeps 3 … 8: 33/6, and with frame 7 (the 8) weighing 10:
///   (25 + 80)/15 = 7.
/// - Winsorized at 2σ drops the 100 of [1, 2, 2, 2, 2, 100]: 9/5, and with frame 0 weighing 10:
///   (10 + 8)/14.
/// - Sigma clipping at 2σ on [2, 100, 3, 2.5, 2.25, 1.75, 2.75, 2.5]: median 2.5, MAD 0.375 drops
///   the 100; then MAD 0.25 → 2σ = 0.74 drops the 1.75. Frame 0 weighs 8 and the rest 1/8:
///   (16 + 13/8)/(8 + 5/8).
/// - Linear fit at 3σ drops the 100 of [1, 1.125, 1.25, 1.375, 100, 1.5] in its seed pass, and
///   the line through the rest is exact: (8 + 5.25/8)/(8 + 4/8).
/// - GESD drops the 100 of [1, 1.125, 0.875, 1, 1.25, 0.75, 1, 100]: (8 + 6/8)/(8 + 6/8) = 1.
#[test]
fn combine_mean_weighs_each_survivor_by_its_own_frame() {
    let ramp = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let tenths: Vec<f32> = (1..=10).map(|i| i as f32).collect();
    let heavy_first = |n: usize, first: f32, rest: f32| {
        let mut weights = vec![rest; n];
        weights[0] = first;
        weights
    };
    let mut heavy_eighth = vec![1.0; 10];
    heavy_eighth[7] = 10.0;
    for (name, rejection, values, weights, expected) in [
        (
            "none",
            Rejection::None,
            &[1.0, 2.0, 3.0, 4.0, 5.0][..],
            vec![1.0; 5],
            3.0,
        ),
        (
            "none, weighted",
            Rejection::None,
            &[1.0, 2.0, 3.0, 4.0, 5.0],
            heavy_first(5, 10.0, 1.0),
            24.0 / 14.0,
        ),
        (
            "sigma clip",
            Rejection::sigma_clip(2.0),
            &ramp,
            vec![1.0; 8],
            2.5,
        ),
        (
            "asymmetric sigma clip, weighted",
            Rejection::sigma_clip_asymmetric(4.0, 2.0),
            &ramp,
            heavy_first(8, 10.0, 1.0),
            26.5 / 16.0,
        ),
        (
            "percentile",
            Rejection::percentile(20.0),
            &tenths,
            vec![1.0; 10],
            5.5,
        ),
        (
            "percentile, weighted",
            Rejection::percentile(20.0),
            &tenths,
            heavy_eighth,
            7.0,
        ),
        (
            "winsorized",
            Rejection::winsorized(2.0),
            &[1.0, 2.0, 2.0, 2.0, 2.0, 100.0],
            vec![1.0; 6],
            1.8,
        ),
        (
            "winsorized, weighted",
            Rejection::winsorized(2.0),
            &[1.0, 2.0, 2.0, 2.0, 2.0, 100.0],
            heavy_first(6, 10.0, 1.0),
            18.0 / 14.0,
        ),
        (
            "sigma clip, weighted",
            Rejection::sigma_clip(2.0),
            &[2.0, 100.0, 3.0, 2.5, 2.25, 1.75, 2.75, 2.5],
            heavy_first(8, 8.0, 0.125),
            (16.0 + 13.0 / 8.0) / (8.0 + 5.0 / 8.0),
        ),
        (
            "linear fit, weighted",
            Rejection::linear_fit(3.0),
            &[1.0, 1.125, 1.25, 1.375, 100.0, 1.5],
            heavy_first(6, 8.0, 0.125),
            (8.0 + 5.25 / 8.0) / (8.0 + 4.0 / 8.0),
        ),
        (
            "gesd, weighted",
            Rejection::Gesd(GesdConfig::new(0.05, Some(3))),
            &[1.0, 1.125, 0.875, 1.0, 1.25, 0.75, 1.0, 100.0],
            heavy_first(8, 8.0, 0.125),
            1.0,
        ),
    ] {
        let mut values = values.to_vec();
        let mean = rejection
            .combine_mean(&mut values, &weights, &mut scratch(), true)
            .value;
        assert_eq!(mean, expected as f32, "{name}");
    }
}

/// Unit weights reduce to the plain mean of the survivors, bit for bit: calibration masters
/// combine with unit weights through this path.
#[test]
fn unit_weights_reduce_to_the_plain_mean_of_the_survivors() {
    let values = [1.0f32, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let mut weighted_values = values;
    let combined = Rejection::sigma_clip(2.0).combine_mean(
        &mut weighted_values,
        &[1.0; 8],
        &mut scratch(),
        true,
    );

    let mut plain_values = values;
    let remaining = Rejection::sigma_clip(2.0).reject(&mut plain_values, &mut scratch());
    let plain = mean_f32(&plain_values[..remaining]);

    assert_eq!(combined.survivor_count, 7);
    assert_eq!(combined.value.to_bits(), plain.to_bits());
}

/// The weighted mean reads each survivor's weight through its frame index.
/// - [2, 4, 6] under [10, 1, 1]: (20 + 4 + 6)/12 = 2.5.
/// - Survivors 10 and 20 of frames 0 and 2, under the three frames' [5, 0.5, 1]: 70/6.
/// - All weights zero: 0, the documented answer for no trusted frame. One non-zero: that value.
/// - 2e7 and sixteen halves: (2e7 + 8)/17, where an f32 running sum drops every half.
#[test]
fn weighted_mean_indexed_reads_weights_by_frame() {
    let mut halves = vec![0.5f32; 17];
    halves[0] = 2.0e7;
    let identity: Vec<usize> = (0..17).collect();
    for (values, weights, indices, expected) in [
        (
            &[2.0, 4.0, 6.0][..],
            &[10.0, 1.0, 1.0][..],
            &[0, 1, 2][..],
            2.5,
        ),
        (&[10.0, 20.0], &[5.0, 0.5, 1.0], &[0, 2], 70.0 / 6.0),
        (&[5.0, 10.0, 15.0], &[0.0; 3], &[0, 1, 2], 0.0),
        (&[5.0, 10.0, 15.0], &[0.0, 2.0, 0.0], &[0, 1, 2], 10.0),
        (&halves, &[1.0; 17], &identity, (2.0e7 + 8.0) / 17.0),
    ] {
        assert_eq!(
            weighted_mean_indexed(values, weights, indices, &mut Vec::new()),
            expected as f32,
            "{values:?} under {weights:?}"
        );
    }
}

/// 100 values in reverse order take the introsort path (past 64): they come out ascending, each
/// index naming the position it came from. A (37·i mod 200) permutation of 200 does too.
#[test]
fn sort_with_indices_carries_every_index_with_its_value() {
    let reversed: Vec<f32> = (0..100).rev().map(|i| i as f32).collect();
    let shuffled: Vec<f32> = (0..200).map(|i| ((i * 37) % 200) as f32).collect();
    for original in [reversed, shuffled] {
        let n = original.len();
        let mut values = original.clone();
        let mut scratch = scratch();
        scratch.reset_indices(n);
        scratch.sort_with_indices(&mut values, n);
        let ascending: Vec<f32> = (0..n).map(|i| i as f32).collect();
        assert_eq!(values, ascending);
        for (value, &index) in values.iter().zip(&scratch.indices) {
            assert_eq!(original[index], *value);
        }
    }
}

/// `reset_indices(n)` leaves exactly `0..n`, whatever was there and however large, and keeps the
/// allocation.
#[test]
fn reset_indices_leaves_the_identity_and_keeps_capacity() {
    for (stale, n) in [
        (vec![], 5),
        (vec![99, 88, 77, 66, 55], 5),
        (vec![1, 2, 3], 0),
        (vec![7; 100], 3),
    ] {
        let mut scratch = ScratchBuffers {
            indices: stale,
            ..Default::default()
        };
        let capacity = scratch.indices.capacity();
        scratch.reset_indices(n);
        assert_eq!(scratch.indices, (0..n).collect::<Vec<_>>());
        assert!(scratch.indices.capacity() >= capacity);
    }
}

/// The early exit: true only when no value can pass `k` trimmed standard deviations, the min and
/// max being left out of the trimmed set. Never below ten values.
///
/// - Twenty 10s, or eighteen with a 9 and an 11: the trimmed set is constant, so nothing can go.
/// - Seventeen 10s with 9, 11 and 100: trimmed mean 181/18, σ 0.236, and the 100 is 90 off.
/// - 0 … 19: trimmed 1 … 18, mean 9.5, σ √(484.5/17) = 5.34; the ends are 9.5 off, inside 13.35.
/// - [1, 1.5, 2, 2.5, 3, 50, 80, 100, 1, 1] at 2: the 80 stays in the trimmed set, but the 100 is
///   82.4 off a mean of 17.6 with σ 30.3, past 2σ.
/// - Five or nine 10s: too few to trim.
#[test]
fn no_outliers_possible_screens_by_trimmed_spread() {
    let with = |base: usize, extra: &[f32]| {
        let mut values = vec![10.0f32; base];
        values.extend_from_slice(extra);
        values
    };
    let ramp: Vec<f32> = (0..20).map(|i| i as f32).collect();
    for (name, values, k, expected) in [
        ("constant", with(20, &[]), 2.5, true),
        ("one each side", with(18, &[11.0, 9.0]), 2.5, true),
        ("far outlier", with(17, &[9.0, 11.0, 100.0]), 2.5, false),
        ("ramp", ramp, 2.5, true),
        (
            "two outliers",
            vec![1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0, 1.0, 1.0],
            2.0,
            false,
        ),
        ("five", with(5, &[]), 2.5, false),
        ("nine", with(9, &[]), 2.5, false),
    ] {
        assert_eq!(
            SigmaClipConfig::no_outliers_possible(&values, k),
            expected,
            "{name}"
        );
    }
}

/// The sigma-clip shortcut decides on `σ² < f32::EPSILON`, a fixed number, so the survivor set
/// depends on the data's scale (review item 1.1). Twenty samples of noise 1e-4 and one hit at 0.5:
/// at unit scale the shortcut keeps the hit, and in ADU (×256) the full path rejects it. The
/// invariance check sees the difference.
#[test]
fn the_sigma_clip_shortcut_depends_on_the_data_scale() {
    use crate::internals::invariance::Affine;

    let mut rng = TestRng::new(11);
    let mut values: Vec<f32> = (0..20)
        .map(|_| Affine::quantize(0.25 + 1e-4 * rng.next_gaussian_f32()))
        .collect();
    values[7] = Affine::quantize(0.5);
    let kept = |case: Affine| {
        survivors(&case.apply_all(&values), |v, s| {
            Rejection::default().reject(v, s)
        })
    };
    assert!(kept(Affine::IDENTITY).contains(&7));
    let failing: Vec<Affine> = Affine::mismatches(kept, |kept, _| kept.clone())
        .into_iter()
        .map(|mismatch| mismatch.case)
        .collect();
    assert!(failing.contains(&Affine::CASES[2]), "{failing:?}");
}
