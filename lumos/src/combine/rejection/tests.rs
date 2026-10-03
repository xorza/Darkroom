use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use statrs::distribution::{Continuous, ContinuousCDF, Normal};

use crate::combine::config::DEFAULT_MIN_SURVIVORS;
use crate::combine::rejection::rejection_scale::RejectionScale;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::*;
use crate::internals::invariance::Affine;
use crate::internals::prelude::*;
use std::f64::consts::TAU;

/// The gather positions `rejection` keeps of `values`, ascending.
fn kept_with(
    rejection: Rejection,
    values: &[f32],
    background: f32,
    min_survivors: usize,
) -> Vec<usize> {
    let mut scratch = ScratchBuffers::default();
    scratch.sorted.fill(values);
    let variances = vec![background * background; values.len()];
    let zeros = vec![0.0; values.len()];
    let noise = NoiseColumns {
        background: &variances,
        sky: &zeros,
        inverse_electrons: &zeros,
    };
    let window = rejection.surviving_window(
        &scratch.sorted,
        Some(noise),
        min_survivors,
        &mut scratch.methods,
    );
    let mut kept: Vec<usize> = scratch.sorted.positions()[window]
        .iter()
        .map(|&position| position as usize)
        .collect();
    kept.sort_unstable();
    kept
}

/// The positions kept with no measured background and the default survivor minimum.
fn kept(rejection: Rejection, values: &[f32]) -> Vec<usize> {
    kept_with(rejection, values, 0.0, DEFAULT_MIN_SURVIVORS)
}

/// Every position of `count` but those in `dropped`.
fn all_but(count: usize, dropped: &[usize]) -> Vec<usize> {
    (0..count).filter(|i| !dropped.contains(i)).collect()
}

/// `combine_mean` over `values` with `weights`, every sample its own frame, and no noise.
fn combine(rejection: Rejection, values: &[f32], weights: &[f32]) -> CombinedSample {
    let mut values = values.to_vec();
    let frame_ids: Vec<u32> = (0..values.len() as u32).collect();
    rejection.combine_mean(
        PixelSamples {
            values: &mut values,
            weights,
            frame_ids: &frame_ids,
            noise: None,
            channel: 0,
        },
        DEFAULT_MIN_SURVIVORS,
        &mut ScratchBuffers::default(),
        true,
    )
}

fn standard_normal(rng: &mut ChaCha8Rng) -> f32 {
    let u1 = rng.random::<f64>().max(f64::MIN_POSITIVE);
    let u2 = rng.random::<f64>();
    ((-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()) as f32
}

/// Every config's documented defaults, its constructors, and the `Rejection` shorthands.
#[test]
fn rejection_configs_default_and_construct_as_documented() {
    let symmetric = SigmaBounds::symmetric;
    assert_eq!(
        SigmaClipConfig::default(),
        SigmaClipConfig {
            sigma: symmetric(2.5),
            max_iterations: 3,
            scale: RejectionScale::Robust,
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
        TrimConfig::default(),
        TrimConfig {
            low_percent: 10.0,
            high_percent: 10.0
        }
    );
    assert_eq!(
        GesdConfig::default(),
        GesdConfig {
            alpha: 0.05,
            max_outliers: None
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
            Rejection::trim(15.0),
            Rejection::Trim(TrimConfig::new(15.0, 15.0)),
        ),
        (Rejection::gesd(), Rejection::Gesd(GesdConfig::default())),
    ] {
        assert_eq!(shorthand, expected);
    }
}

/// The automatic GESD cap is `⌊0.3·n⌋`, in integers; an explicit one is taken as given.
#[test]
fn the_gesd_cap_is_three_tenths_of_the_samples() {
    for (count, expected) in [(0, 0), (3, 0), (4, 1), (10, 3), (20, 6), (25, 7), (100, 30)] {
        assert_eq!(
            GesdConfig::default().max_outliers_for_size(count),
            expected,
            "{count}"
        );
    }
    assert_eq!(
        GesdConfig::new(0.05, Some(11)).max_outliers_for_size(44),
        11
    );
}

/// Which samples sigma clipping keeps, by hand. σ = 1.4826 · MAD · `b_n` about the median.
///
/// - `ramp`, [1, 1.5, …, 4, 100]: median 2.75, deviations ranked .25 .25 .75 .75 1.25 1.25 1.75
///   97.25, MAD 1, σ = 1.4826 · 1.1274 = 1.671. At 2σ the band reaches 6.09 and drops the 100.
///   The seven left: median 2.5, MAD 1, σ = 1.4826 · 1.1378 = 1.687, and the band holds all. The
///   same at 2.5σ, and with low 4σ, high 2σ.
/// - `low_kept`, [−5, 1, 1.5, …, 4.5, 50] at low 10σ, high 2σ: median 2.75, MAD 1.25, σ 2.031;
///   the high cut 6.81 drops the 50, the low cut −17.6 keeps the −5. Then median 2.5, MAD 1, σ
///   1.632, and both cuts keep the rest.
/// - `three_high`, [1, 1.5, 2, 2.5, 3, 50, 80, 100] at 2σ: median 2.75, MAD 1.5, σ 2.507, so the
///   band reaches 7.76: the three high values go. The five left: median 2, MAD 0.5, σ 0.902,
///   band [0.20, 3.80], all kept.
/// - `clean`, [1, 1.125, 1.25, 0.875, 1.0625] at 3σ: median 1.0625, MAD 0.0625, σ 0.1128, band
///   [0.724, 1.401]: all kept.
/// - `bright`, fourteen values 7990 … 8010 about 8000 and a 9000 at 2.5σ: median 8000, MAD 5, σ
///   7.83, band ±19.6: the 9000 goes.
/// - `two_levels`, 47 × 9, 47 × 11 and six values from 100 to 800 at 2.5σ: median 11, MAD 2 (rank
///   50 of the deviations is a 2), σ 2.99, band [3.53, 18.47]: the six go. Then median 10, MAD 1,
///   σ 1.50, band [6.26, 13.74]: the rest stay.
/// - `hot`, nineteen samples of σ about 0.001 at 0.5 and a hot pixel at 10⁶: the resolution floor
///   is taken at the centre, 0.5·ε = 6e-8, not at the largest sample, where it would be 0.12 and
///   hold the cluster's own spread under it; the pixel goes.
/// - Three samples are no more than the survivor minimum: nothing goes.
#[test]
fn sigma_clip_keeps_exactly_the_samples_its_bands_hold() {
    let ramp = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let two_levels: Vec<f32> = [9.0; 47]
        .into_iter()
        .chain([11.0; 47])
        .chain([100.0, 200.0, 500.0, 600.0, 700.0, 800.0])
        .collect();
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    let mut hot: Vec<f32> = (0..19)
        .map(|_| 0.5 + 1e-3 * standard_normal(&mut rng))
        .collect();
    hot.push(1e6);
    for (name, values, config, kept_positions) in [
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
            &[-5.0, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 50.0],
            SigmaClipConfig::new_asymmetric(10.0, 2.0, 5),
            all_but(10, &[9]),
        ),
        (
            "three high",
            &[1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0],
            SigmaClipConfig::new(2.0, 3),
            all_but(8, &[5, 6, 7]),
        ),
        (
            "clean",
            &[1.0, 1.125, 1.25, 0.875, 1.0625],
            SigmaClipConfig::new(3.0, 3),
            all_but(5, &[]),
        ),
        (
            "bright",
            &[
                7990.0, 8000.0, 8010.0, 7995.0, 8005.0, 8000.0, 7990.0, 8010.0, 8000.0, 7995.0,
                8005.0, 8000.0, 7990.0, 8010.0, 9000.0,
            ],
            SigmaClipConfig::new(2.5, 3),
            all_but(15, &[14]),
        ),
        (
            "two levels",
            &two_levels,
            SigmaClipConfig::new(2.5, 3),
            all_but(100, &[94, 95, 96, 97, 98, 99]),
        ),
        (
            "hot",
            &hot,
            SigmaClipConfig::new(2.5, 3),
            all_but(20, &[19]),
        ),
        (
            "three samples",
            &[1.0, 2.0, 100.0],
            SigmaClipConfig::default(),
            all_but(3, &[]),
        ),
    ] {
        assert_eq!(
            kept(Rejection::SigmaClip(config), values),
            kept_positions,
            "{name}"
        );
    }
    // The asymmetric form at equal thresholds is the symmetric one.
    assert_eq!(
        kept(Rejection::sigma_clip_asymmetric(2.5, 2.5), &ramp),
        kept(Rejection::sigma_clip(2.5), &ramp)
    );
}

/// Review example 1.2, where the old shortcut's non-robust σ let two outliers hide each other:
/// {−0.1, −0.05, 0, 0, 0, 0, 0.05, 0.1, 10, 10} at 2.5σ.
///
/// Pass 1: median 0, deviations ranked 0 0 0 0 .05 .05 .1 .1 10 10, MAD 0.05, σ = 0.05 · 1.4826 ·
/// 1.0958 = 0.0812, band ±0.203: both 10s go. Pass 2: MAD (0 + 0.05)/2 = 0.025, σ = 0.025 · 1.4826
/// · 1.1274 = 0.0418, band ±0.1045, which holds the ±0.1: nothing more goes. Without the
/// small-sample factor the band would be ±0.0927 and the ±0.1 would go too.
#[test]
fn two_outliers_do_not_hide_each_other() {
    let values = [-0.1, -0.05, 0.0, 0.0, 0.0, 0.0, 0.05, 0.1, 10.0, 10.0];
    assert_eq!(
        kept(Rejection::sigma_clip(2.5), &values),
        all_but(10, &[8, 9])
    );
}

/// Integer bias frames with a measured background of 0.7 ADU, scaled by 2⁻¹⁶ as a 16-bit decode
/// leaves them: more than half the samples tie at 1000, so the MAD is 0, and the old rejection
/// stopped there and kept the hit. The floor is the background, so the band is ±2.5 · 0.7 = ±1.75
/// ADU about 1000: it keeps the 999s, 1000s and 1001s, and drops the 1002 and the hit at 1050. The
/// next pass has the same median and floor and drops nothing more.
#[test]
fn the_background_floors_a_window_of_tied_integers() {
    let adu = [
        1000.0, 1000.0, 1001.0, 1000.0, 999.0, 1000.0, 1000.0, 1001.0, 1000.0, 1000.0, 999.0,
        1000.0, 1000.0, 1002.0, 1001.0, 1000.0, 1000.0, 999.0, 1000.0, 1050.0,
    ];
    let scale = 1.0 / 65_536.0;
    let values: Vec<f32> = adu.iter().map(|&value: &f32| value * scale).collect();
    assert_eq!(
        kept_with(
            Rejection::sigma_clip(2.5),
            &values,
            0.7 * scale,
            DEFAULT_MIN_SURVIVORS
        ),
        all_but(20, &[13, 19])
    );
    // With no background the band about the tied 1000s holds them alone.
    assert_eq!(
        kept(Rejection::sigma_clip(2.5), &values),
        (0..20).filter(|&i| adu[i] == 1000.0).collect::<Vec<_>>()
    );
}

/// Review example 1.1, twenty samples of noise 10⁻⁴ about 0.25 and a hit at 0.5, decides the same
/// under every exact affine map of the data, for every method, with the background mapped with
/// the data. The old sigma-clip shortcut kept the hit at unit scale and dropped it in ADU.
#[test]
fn every_method_decides_the_same_at_every_scale() {
    let mut rng = TestRng::new(11);
    let mut values: Vec<f32> = (0..20)
        .map(|_| Affine::quantize(0.25 + 1e-4 * rng.next_gaussian_f32()))
        .collect();
    values[7] = Affine::quantize(0.5);
    for rejection in [
        Rejection::sigma_clip(2.5),
        Rejection::winsorized(2.5),
        Rejection::linear_fit(3.0),
        Rejection::gesd(),
        Rejection::trim(10.0),
    ] {
        for background in [0.0, 1e-4] {
            let kept_under = |case: Affine| {
                kept_with(
                    rejection,
                    &case.apply_all(&values),
                    background * case.scale,
                    DEFAULT_MIN_SURVIVORS,
                )
            };
            assert!(
                !kept_under(Affine::IDENTITY).contains(&7),
                "{rejection:?} kept the hit"
            );
            Affine::assert_equivariant(kept_under, |kept, _| kept.clone());
        }
    }
}

/// Which samples linear-fit clipping keeps. Its first pass is the sigma clip; the passes after it
/// fit the kept samples against their normal scores among all of the pixel's samples.
///
/// - `off_line`, [1, 2, 3, 4, 100, 6] at 2σ: median 3.5, MAD 1.5, σ = 1.5 · 1.4826 · 1.1895 =
///   2.65, so the first pass drops the 100; the five left lie close to their normal scores.
/// - `top`, [1 … 7, 100], and `middle`, the same with the 100 a 50 at position 3: median 4.5, MAD
///   2, σ 3.34; the first pass drops the outlier.
/// - `one_pass`, [10, 10.5, 11, 10.2, 10.8, 10.3, 10.7, 50] with one fitted pass at 3σ: median
///   10.6, MAD 0.35, σ 0.585: the first pass drops the 50.
/// - `constant` has no spread; `trend` [1, 3, …, 15], evenly spaced, and `ramp` 10 … 90 with a 5
///   sit within 2σ of their fitted lines at both ends; three samples are no more than the
///   minimum: nothing goes.
/// - `long`, 100 samples on `y = x` with sample 50 at 1000: the first pass drops it, and the rest,
///   evenly spaced, stay within 3σ of their fit.
/// - `fit_only`, [−1, −0.5, −0.5, −0.5, 0, 0.5, 0.5, 0.5, 1, 1.5, 5.25] at 3σ: the median clip
///   (median 0.5, MAD 1, σ 1.601, band up to 5.30) keeps the 5.25, and so does sigma clipping to
///   its end. The first fit over all eleven has the scores summing to 0, so the centre is the mean
///   0.6136, and the slope 1.542 puts the band's top at 5.240: the 5.25 goes. The fit over the ten
///   left, centre 0.296 and slope 0.918, keeps [−2.46, 3.05].
#[test]
fn linear_fit_keeps_exactly_the_samples_near_its_line() {
    let fit_only = [-1.0, -0.5, -0.5, -0.5, 0.0, 0.5, 0.5, 0.5, 1.0, 1.5, 5.25];
    assert_eq!(
        kept(Rejection::sigma_clip(3.0), &fit_only),
        all_but(11, &[])
    );
    let mut long: Vec<f32> = (0..100).map(|i| i as f32).collect();
    long[50] = 1000.0;
    for (name, values, config, kept_positions) in [
        (
            "off line",
            &[1.0, 2.0, 3.0, 4.0, 100.0, 6.0][..],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(6, &[4]),
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
            "ramp",
            &[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 5.0],
            LinearFitClipConfig::new(2.0, 2.0, 3),
            all_but(10, &[]),
        ),
        (
            "three samples",
            &[1.0, 2.0, 100.0],
            LinearFitClipConfig::default(),
            all_but(3, &[]),
        ),
        (
            "long",
            &long,
            LinearFitClipConfig::new(3.0, 3.0, 3),
            all_but(100, &[50]),
        ),
        (
            "fit only",
            &fit_only,
            LinearFitClipConfig::new(3.0, 3.0, 3),
            all_but(11, &[10]),
        ),
    ] {
        assert_eq!(
            kept(Rejection::LinearFit(config), values),
            kept_positions,
            "{name}"
        );
    }
}

/// The fit is exact on samples that lie on their normal scores: `y = 2 + 0.5·z` gives centre 2
/// and σ 0.5, up to rounding. The samples are f32, off by up to half an ulp near 2.6, 1.2e-7; the
/// fit carries that into the centre at most once, and into the slope at most `Σ|z − z̄|/Σ(z − z̄)²`
/// = 0.94 times. Rounding the results to f32 adds half an ulp at 2 (1.2e-7) and at 0.5 (3e-8).
#[test]
fn the_fit_reads_the_centre_and_sigma_off_the_normal_scores() {
    let mut scores = normal_scores::NormalScores::default();
    let z = scores.of_count(9).to_vec();
    let values: Vec<f32> = z.iter().map(|&z| (2.0 + 0.5 * z) as f32).collect();
    let positions: Vec<u32> = (0..9).collect();
    let pass = Pass {
        sorted: &values,
        positions: &positions,
        noise: None,
        window: 0..9,
        index: 1,
        background: 0.0,
        min_survivors: DEFAULT_MIN_SURVIVORS,
    };
    let fit = LinearFitClipConfig::fit(&pass, &mut scores);
    assert!((fit.centre - 2.0).abs() <= 2.4e-7, "{fit:?}");
    assert!((fit.sigma - 0.5).abs() <= 1.5e-7, "{fit:?}");
}

/// On clean Gaussian samples at 3σ the rejected share falls with the count, toward the Gaussian
/// tail share of 0.27%; Siril's rank-based fit grows with it instead (4.4%, 6.3% and 7.8% at 20,
/// 50 and 200 samples, review 1.5).
///
/// The expected shares come from the SciPy reference `internals/reference/linear_fit_rate.py` over
/// 10⁶, 4·10⁵ and 10⁵ trials: 1.1638%, 0.6147% and 0.3769%, with per-trial standard
/// deviations of the rejected count of 0.615, 0.681 and 0.967. The tolerance is four standard
/// errors of the difference between that reference and this run.
#[test]
fn linear_fit_rejects_clean_data_at_a_rate_that_falls_with_the_count() {
    let mut rng = ChaCha8Rng::seed_from_u64(0x51ed_270b_9a4c_33e1);
    let mut scratch = ScratchBuffers::default();
    let mut previous = f64::INFINITY;
    for (count, trials, reference, per_trial_sd, reference_trials) in [
        (20usize, 5000usize, 0.011_638, 0.615, 1_000_000.0),
        (50, 2000, 0.006_147, 0.681, 400_000.0),
        (200, 500, 0.003_769, 0.967, 100_000.0),
    ] {
        let mut rejected = 0usize;
        let mut values = vec![0.0f32; count];
        for _ in 0..trials {
            for value in &mut values {
                *value = standard_normal(&mut rng);
            }
            scratch.sorted.fill(&values);
            let window = Rejection::linear_fit(3.0).surviving_window(
                &scratch.sorted,
                None,
                DEFAULT_MIN_SURVIVORS,
                &mut scratch.methods,
            );
            rejected += count - window.len();
        }
        let share = rejected as f64 / (count * trials) as f64;
        let count_f = count as f64;
        let standard_error =
            (per_trial_sd / count_f) * (1.0 / trials as f64 + 1.0 / reference_trials).sqrt();
        assert!(
            (share - reference).abs() <= 4.0 * standard_error,
            "{count} samples: rejected {share}, reference {reference} ± {standard_error}"
        );
        assert!(
            share < previous,
            "{count} samples: {share} after {previous}"
        );
        previous = share;
    }
}

/// Which samples winsorized clipping keeps. The estimate starts from the median and the MAD σ,
/// clamps at ±1.5σ, and takes the mean and the corrected standard deviation of the clamped copy.
///
/// - `ramp`, [1, 1.5, …, 4, 100] at 2σ, and `high`, [1, 1.1, 1.2, 0.9, 1, 50] at low 3σ, high
///   2σ: the clamp pulls the outlier in to 1.5σ, and the band about the estimate drops it.
/// - `clean`, [2, 2.125, 2.25, 1.875, 2.0625] at 3σ: all kept.
/// - `mild`, [1, 1.1, 1.2, 0.9, 1, 1.1, 0.8, 1.3, 2]: the 2.0 is kept at 10σ and dropped at 2σ.
/// - `three_of_ten`, seven samples from −1.5 to 1 and three at 10 (σ of the seven 0.85, so the
///   three sit near 12σ) at 3σ: median 0.25, MAD 1, σ 1.62; the first clamp holds the 10s at
///   2.69, and the clamped copy, never released, keeps them there. The band drops all three. The
///   old start, 1.134 × the plain standard deviation, put all three inside the clamp (review 1.3).
/// - Three samples are no more than the minimum.
#[test]
fn winsorized_keeps_exactly_the_samples_its_band_holds() {
    let mild = [1.0, 1.1, 1.2, 0.9, 1.0, 1.1, 0.8, 1.3, 2.0];
    for (name, values, config, kept_positions) in [
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
            "three of ten",
            &[-1.5, -1.0, -0.5, 0.0, 0.0, 0.5, 1.0, 10.0, 10.0, 10.0],
            WinsorizedClipConfig::new(3.0),
            all_but(10, &[7, 8, 9]),
        ),
        (
            "three samples",
            &[1.0, 2.0, 100.0],
            WinsorizedClipConfig::default(),
            all_but(3, &[]),
        ),
    ] {
        assert_eq!(
            kept(Rejection::Winsorized(config), values),
            kept_positions,
            "{name}"
        );
    }
}

/// The estimate by hand on [9.8, 9.9, 10, 10, 10.1, 10.1, 10.2, 10.3, 50]: median 10.1, MAD 0.1,
/// σ₀ = 0.1 · 1.4826 · 1.1011 = 0.16325. The clamp to 10.1 ± 0.24487 moves the 9.8 up to 9.85513
/// and the 50 down to 10.34487, symmetric about 10.1, so the mean is 90.8/9 = 10.08889 and σ₁ is
/// 1.133393 × the standard deviation of the clamped nine, 0.191681. The next clamp, 10.08889 ±
/// 0.28752, holds every clamped value, so nothing moves and σ is converged. The samples are f32
/// roundings of decimals, off by up to 5e-7; that moves σ by less than 1e-5, the tolerance.
#[test]
fn the_winsorized_estimate_converges_on_its_clamped_copy() {
    let sorted = [9.8, 9.9, 10.0, 10.0, 10.1, 10.1, 10.2, 10.3, 50.0];
    let estimate = WinsorizedClipConfig::estimate(&sorted, 0.0, &mut Vec::new());
    assert!((estimate.centre - 90.8 / 9.0).abs() < 1e-5, "{estimate:?}");
    let clamped = [
        9.855_126, 9.9, 10.0, 10.0, 10.1, 10.1, 10.2, 10.3, 10.344_874,
    ];
    let mean = 90.8f64 / 9.0;
    let squares: f64 = clamped.iter().map(|&v: &f64| (v - mean).powi(2)).sum();
    let expected = 1.133_392_655_462_487 * (squares / 8.0).sqrt();
    assert!(
        (f64::from(estimate.sigma) - expected).abs() < 1e-5,
        "{estimate:?} against {expected}"
    );
}

/// The correction is `1/√v` with `v` the variance of a unit Gaussian clamped to ±1.5:
/// `(2Φ(1.5) − 1) − 3φ(1.5) + 4.5(1 − Φ(1.5))`, through statrs. mpmath at 30 digits gives
/// 1.13339265546248702, which the constant holds to its last bit; statrs's normal cdf is off by
/// about 5e-12 here, so 1e-11 is the tolerance.
#[test]
fn the_winsorized_correction_is_the_clamped_gaussian_variance() {
    let normal = Normal::standard();
    let c = 1.5;
    let variance =
        (2.0 * normal.cdf(c) - 1.0) - 2.0 * c * normal.pdf(c) + 2.0 * c * c * (1.0 - normal.cdf(c));
    let correction = 1.0 / variance.sqrt();
    assert!(
        (correction - 1.133_392_655_462_487).abs() < 1e-11,
        "{correction}"
    );
}

/// A trim drops `⌊p·n/100⌋` from each end, exactly. [5, 1, 3, 2, 4] at 20% drops one each side,
/// keeping positions 2, 3 and 4. 42% of 150 is 63, where `(0.42)·150` in f32 is 62.99999 and
/// floors to 62. 49% of 5 drops two each side and leaves one sample, which the driver raises to
/// the three nearest the median.
#[test]
fn trim_drops_exact_counts_from_each_end() {
    assert_eq!(
        kept(Rejection::trim(20.0), &[5.0, 1.0, 3.0, 2.0, 4.0]),
        [2, 3, 4]
    );
    assert_eq!(
        TrimConfig::new(42.0, 10.0).counts(150),
        trim_config::TrimCounts { low: 63, high: 15 }
    );
    assert_eq!((0.42f32 * 150.0).floor(), 62.0);
    for (percent, count, expected) in [(20.0, 10, 2), (49.0, 5, 2), (10.0, 1, 0), (12.5, 8, 1)] {
        assert_eq!(
            TrimConfig::new(percent, percent).counts(count).low,
            expected,
            "{percent}% of {count}"
        );
    }
    assert_eq!(
        kept(Rejection::trim(49.0), &[1.0, 2.0, 3.0, 4.0, 10.0]),
        [1, 2, 3]
    );
}

/// GESD drops a single bright outlier and keeps sets without one: a constant set, a tight one,
/// and three samples, no more than the minimum.
#[test]
fn gesd_drops_an_outlier_and_keeps_clean_sets() {
    assert_eq!(
        kept(
            Rejection::gesd(),
            &[1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0]
        ),
        all_but(8, &[7])
    );
    for (name, values) in [
        ("constant", &[1.0; 8][..]),
        ("tight", &[1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1]),
        ("three samples", &[1.0, 2.0, 100.0]),
    ] {
        assert_eq!(
            kept(Rejection::gesd(), values),
            all_but(values.len(), &[]),
            "{name}"
        );
    }
}

/// Seventeen clean samples, −1.6 … 1.6 in steps of 0.2, and three at 10. With all twenty the mean
/// is 30/20 = 1.5 and the sum of squares 16.32 + 17·2.25 + 3·8.5² = 271.32, so the first statistic
/// is 8.5/√(271.32/19) = 2.249, under its critical value 2.708: the three mask each other. The
/// second removal reaches 2.717 against 2.681, and the third 3.700 against 2.652. The automatic cap
/// ⌊0.3·20⌋ = 6 reaches the third and drops all three. The old cap of 2 stopped at the second and
/// kept one 10 (review 1.6).
#[test]
fn gesd_tests_far_enough_to_unmask_three_outliers() {
    let mut values: Vec<f32> = (0..17).map(|i| -1.6 + 0.2 * i as f32).collect();
    values.extend([10.0; 3]);
    assert_eq!(kept(Rejection::gesd(), &values), all_but(20, &[17, 18, 19]));
    assert_eq!(
        kept(Rejection::Gesd(GesdConfig::new(0.05, Some(2))), &values),
        all_but(20, &[18, 19])
    );
}

/// NIST's worked example (Engineering Statistics Handbook §1.3.5.17.3): 54 values, ten candidates
/// at α = 0.05, three outliers. The handbook prints each statistic and critical value cut to three
/// decimals, so each computed one is the printed one plus less than 0.001.
#[test]
fn gesd_matches_nist_reference_example() {
    let values = [
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
    let mut scratch = ScratchBuffers::default();
    scratch.sorted.fill(&values);
    let window = Rejection::Gesd(GesdConfig::new(0.05, Some(10))).surviving_window(
        &scratch.sorted,
        None,
        DEFAULT_MIN_SURVIVORS,
        &mut scratch.methods,
    );
    assert_eq!(window, 0..51);
    let gesd = &mut scratch.methods.gesd;
    assert_eq!(gesd.statistics.len(), 10);
    for (removed, (expected_statistic, expected_critical)) in expected.into_iter().enumerate() {
        let statistic = gesd.statistics[removed];
        let critical = gesd.critical_value(54 - removed);
        for (computed, printed) in [
            (statistic, expected_statistic),
            (critical, expected_critical),
        ] {
            assert!(
                (0.0..0.001).contains(&(computed - printed)),
                "{computed} does not cut to {printed}"
            );
        }
    }
}

/// At an α no t quantile resolves, the critical value is the finite limit `(L − 1)/√L`.
#[test]
fn gesd_tiny_alpha_uses_the_limiting_critical_value() {
    let mut gesd = scratch_buffers::GesdScratch::default();
    gesd.prepare(f32::MIN_POSITIVE);
    assert_eq!(gesd.critical_value(15), 14.0 / 15.0f64.sqrt());
}

/// Mirrored samples drop the mirrored outliers.
#[test]
fn gesd_is_sign_symmetric() {
    let values = [
        -1.4, -1.2, -1.0, -0.8, -0.6, -0.4, -0.2, 0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2, 1.4, -8.0,
        10.0,
    ];
    let mirrored = values.map(|value: f32| -value);
    let rejection = Rejection::Gesd(GesdConfig::new(0.05, Some(2)));
    assert_eq!(kept(rejection, &values), all_but(17, &[15, 16]));
    assert_eq!(kept(rejection, &mirrored), all_but(17, &[15, 16]));
}

/// On clean Gaussian samples the test drops anything in a share α of the sets: the per-set false
/// positive rate the critical values are built for. Five standard errors of the binomial share
/// over 4000 sets is the tolerance.
#[test]
fn gesd_gaussian_false_positive_rate_matches_alpha() {
    const ALPHA: f32 = 0.05;
    const TRIALS: usize = 4_000;
    // Not `TestRng`: it is an LCG, and Box-Muller over consecutive LCG outputs lays the pairs on a
    // handful of spirals rather than filling the plane. That distorts the tails, which is exactly
    // what an outlier test measures.
    let mut rng = ChaCha8Rng::seed_from_u64(0x947e_4d3a_7c16_b205);
    let mut scratch = ScratchBuffers::default();
    for count in [15, 25, 50, 100] {
        let rejection = Rejection::Gesd(GesdConfig::new(ALPHA, Some(count / 4)));
        let mut false_positives = 0usize;
        let mut values = vec![0.0f32; count];
        for _ in 0..TRIALS {
            for value in &mut values {
                *value = standard_normal(&mut rng);
            }
            scratch.sorted.fill(&values);
            let window = rejection.surviving_window(
                &scratch.sorted,
                None,
                DEFAULT_MIN_SURVIVORS,
                &mut scratch.methods,
            );
            if window.len() < count {
                false_positives += 1;
            }
        }
        let actual = false_positives as f64 / TRIALS as f64;
        let expected = f64::from(ALPHA);
        let standard_error = (expected * (1.0 - expected) / TRIALS as f64).sqrt();
        assert!(
            (actual - expected).abs() <= 5.0 * standard_error,
            "n = {count}: expected false-positive rate {expected}, got {actual}"
        );
    }
}

/// No method leaves fewer than the minimum. When a pass proposes fewer, the driver keeps the
/// minimum nearest the pass's centre, dropping the farther end first and the higher end on a tie.
///
/// - Eight 1s, then 5, 6, 7, with no background: the band about the tied 1s holds them alone,
///   eight samples. With a minimum of 9 the driver keeps the eight 1s and the 5, the nearest of the
///   rest to the centre 1.
/// - A trim of 49% of [1, 2, 3, 4, 10] leaves one sample; the three nearest the median 3 are 2, 3
///   and 4: the 10 (7 off) goes before the 1 (2 off), and then the 1 before the 4 (1 off).
/// - [1, 2, 3, 4] about 2.5 with a minimum of 3: the 1 and the 4 tie at 1.5 off, so the 4 goes.
#[test]
fn the_driver_keeps_the_minimum_nearest_the_centre() {
    let mut tied = vec![1.0f32; 8];
    tied.extend([5.0, 6.0, 7.0]);
    assert_eq!(
        kept_with(Rejection::sigma_clip(2.5), &tied, 0.0, 3),
        all_but(11, &[8, 9, 10])
    );
    assert_eq!(
        kept_with(Rejection::sigma_clip(2.5), &tied, 0.0, 9),
        all_but(11, &[9, 10])
    );
    assert_eq!(nearest(&[1.0, 2.0, 3.0, 4.0], 0..4, 2.5, 3), 0..3);
    assert_eq!(nearest(&[1.0, 2.0, 3.0, 4.0, 10.0], 0..5, 3.0, 3), 1..4);

    let mut rng = ChaCha8Rng::seed_from_u64(17);
    for minimum in 1..=6 {
        for rejection in [
            Rejection::sigma_clip(0.5),
            Rejection::winsorized(0.5),
            Rejection::linear_fit(0.5),
            Rejection::Gesd(GesdConfig::new(0.5, Some(10))),
            Rejection::trim(45.0),
        ] {
            for _ in 0..20 {
                let values: Vec<f32> = (0..10).map(|_| standard_normal(&mut rng)).collect();
                let survivors = kept_with(rejection, &values, 0.0, minimum).len();
                assert!(
                    survivors >= minimum,
                    "{rejection:?} kept {survivors} of 10 under {minimum}"
                );
            }
        }
    }
}

/// `combine_mean` averages the survivors under their own weights. Each expected value is the
/// weighted mean of survivors derived above, an exact quotient rounded once.
///
/// - No rejection over 1 … 5: 15/5, and with frame 0 weighing 10: (10 + 14)/14.
/// - Sigma clipping at 2σ drops the 100 of the ramp: 17.5/7. At low 4σ, high 2σ with frame 0
///   weighing 10: (10 + 16.5)/16.
/// - A 20% trim of 1 … 10 keeps 3 … 8: 33/6, and with frame 7 (the 8) weighing 10: (25 + 80)/15.
/// - Winsorized at 2σ drops the 100 of [1, 1.25, 1.5, 1.75, 2, 100]: 7.5/5, and with frame 0
///   weighing 10: (10 + 6.5)/14.
/// - Sigma clipping at 2σ on [2, 100, 3, 2.5, 2.25, 1.75, 2.75, 2.5]: median 2.5, MAD 0.375, σ
///   0.627 drops the 100; then median 2.5, MAD 0.25, σ 0.426, band [1.65, 3.35] keeps the rest.
///   Frame 0 weighs 8 and the rest 1/8: (16 + 14.75/8)/(8 + 6/8).
/// - Linear fit at 3σ drops the 100 of [1, 1.125, 1.25, 1.375, 100, 1.5] in its first pass:
///   (8 + 5.25/8)/(8 + 4/8).
/// - GESD drops the 100 of [1, 1.125, 0.875, 1, 1.25, 0.75, 1, 100]: (8 + 6/8)/(8 + 6/8) = 1.
#[test]
fn combine_mean_weighs_each_survivor_by_its_own_weight() {
    let ramp = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let tenths: Vec<f32> = (1..=10).map(|i| i as f32).collect();
    let heavy_first = |n: usize, first: f32, rest: f32| {
        let mut weights = vec![rest; n];
        weights[0] = first;
        weights
    };
    let mut heavy_eighth = vec![1.0; 10];
    heavy_eighth[7] = 10.0;
    let quarters = [1.0, 1.25, 1.5, 1.75, 2.0, 100.0];
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
        ("trim", Rejection::trim(20.0), &tenths, vec![1.0; 10], 5.5),
        (
            "trim, weighted",
            Rejection::trim(20.0),
            &tenths,
            heavy_eighth,
            7.0,
        ),
        (
            "winsorized",
            Rejection::winsorized(2.0),
            &quarters,
            vec![1.0; 6],
            1.5,
        ),
        (
            "winsorized, weighted",
            Rejection::winsorized(2.0),
            &quarters,
            heavy_first(6, 10.0, 1.0),
            16.5 / 14.0,
        ),
        (
            "sigma clip, weighted",
            Rejection::sigma_clip(2.0),
            &[2.0, 100.0, 3.0, 2.5, 2.25, 1.75, 2.75, 2.5],
            heavy_first(8, 8.0, 0.125),
            (16.0 + 14.75 / 8.0) / (8.0 + 6.0 / 8.0),
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
        assert_eq!(
            combine(rejection, values, &weights).value,
            expected as f32,
            "{name}"
        );
    }
}

/// The survivors' positions follow the sort: the ramp's survivors are its first seven samples, in
/// ascending order, and a pixel that sorted nothing reports none. The sample counts them, and its
/// weight is that of the seven unit weights, 7.
///
/// The variance is `Σw²·v/(Σw)²` with each sample's model taken at the combined value 17.5/7 =
/// 2.5: background 1/4 plus `(2.5 − 1/2)·1/4` above the sky, 3/4, so 7·(3/4)/49 = 3/28, exact up
/// to its one rounding.
#[test]
fn the_survivors_are_named_after_the_combine() {
    let ramp = [4.0, 1.0, 100.0, 3.5, 1.5, 2.0, 3.0, 2.5];
    let mut values = ramp;
    let frame_ids: Vec<u32> = (0..8).collect();
    let noise = NoiseColumns {
        background: &[0.25; 8],
        sky: &[0.5; 8],
        inverse_electrons: &[0.25; 8],
    };
    let mut scratch = ScratchBuffers::default();
    let sample = Rejection::sigma_clip(2.0).combine_mean(
        PixelSamples {
            values: &mut values,
            weights: &[1.0; 8],
            frame_ids: &frame_ids,
            noise: Some(noise),
            channel: 0,
        },
        DEFAULT_MIN_SURVIVORS,
        &mut scratch,
        true,
    );
    assert_eq!(sample.survivor_count, 7);
    assert_eq!(
        scratch.survivor_positions(),
        Some(&[1, 4, 5, 7, 6, 3, 0][..])
    );
    assert_eq!(sample.value, 2.5);
    assert_eq!(sample.weight, 7.0);
    assert_eq!(sample.variance, 3.0 / 28.0);

    Rejection::None.combine_mean(
        PixelSamples {
            values: &mut values,
            weights: &[1.0; 8],
            frame_ids: &frame_ids,
            noise: None,
            channel: 0,
        },
        DEFAULT_MIN_SURVIVORS,
        &mut scratch,
        true,
    );
    assert_eq!(scratch.survivor_positions(), None);
}

/// The background columns become the floor as their root mean square: four samples of variance
/// 0.25 and four of 0.75 give a background of √0.5 = 0.7071. On [0, 0, 0, 0, 0, 0, 0, 1.5], whose
/// MAD is 0, the band at 2σ is ±1.414, so the 1.5 goes; with no noise gathered the band about the
/// tied zeros is one subnormal step wide and the 1.5 goes all the same, and with variances of 1
/// the band is ±2 and keeps it.
#[test]
fn the_gathered_noise_floors_the_spread() {
    let values = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.5];
    let frame_ids: Vec<u32> = (0..8).collect();
    let zeros = [0.0; 8];
    let survivors = |background: Option<&[f32]>| {
        let mut values = values;
        Rejection::sigma_clip(2.0)
            .combine_mean(
                PixelSamples {
                    values: &mut values,
                    weights: &[1.0; 8],
                    frame_ids: &frame_ids,
                    noise: background.map(|background| NoiseColumns {
                        background,
                        sky: &zeros,
                        inverse_electrons: &zeros,
                    }),
                    channel: 0,
                },
                DEFAULT_MIN_SURVIVORS,
                &mut ScratchBuffers::default(),
                false,
            )
            .survivor_count
    };
    let mixed = [0.25, 0.25, 0.25, 0.25, 0.75, 0.75, 0.75, 0.75];
    assert_eq!(survivors(Some(&mixed)), 7);
    assert_eq!(survivors(None), 7);
    assert_eq!(survivors(Some(&[1.0; 8])), 8);
}

/// On [99, 100, 100, 101, 100, 115] at 2.5σ the two scales disagree. The MAD about the median 100
/// is 0.5, so σ = 0.5 · 1.4826 · 1.1895 = 0.882 and the band ±2.2 drops the 115. The five left
/// have a MAD of 0, and the measured background, √0.01 = 0.1, makes the band ±0.25: the 99 and the
/// 101 go too, and three samples stay. The CCD model with background variance 0.01, sky 0 and one
/// electron per unit gives 0.01 + 100 = 100.01 at the median, σ 10.0, band ±25: the 115 is 1.5σ of
/// photon noise above 100 electrons, and all six stay.
#[test]
fn the_ccd_model_reads_photon_noise_the_mad_cannot_see() {
    let values = [99.0, 100.0, 100.0, 101.0, 100.0, 115.0];
    let background = [0.01; 6];
    let sky = [0.0; 6];
    let inverse_electrons = [1.0; 6];
    let noise = NoiseColumns {
        background: &background,
        sky: &sky,
        inverse_electrons: &inverse_electrons,
    };
    let kept_under = |scale| {
        let mut scratch = ScratchBuffers::default();
        scratch.sorted.fill(&values);
        let rejection = Rejection::SigmaClip(SigmaClipConfig::new(2.5, 3).with_scale(scale));
        rejection
            .surviving_window(
                &scratch.sorted,
                Some(noise),
                DEFAULT_MIN_SURVIVORS,
                &mut scratch.methods,
            )
            .len()
    };
    assert_eq!(kept_under(RejectionScale::Robust), 3);
    assert_eq!(kept_under(RejectionScale::CcdModel), 6);
}
