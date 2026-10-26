use std::slice;

use common::{Buffer2, CancelToken, TempDir};
use glam::Vec2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::combine::frame_cache::FrameCache;
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::common_domain::{ChannelCoverage, CommonDomain};
use crate::combine::normalization::photometric_gain::{
    MAD_SIGMA_VARIANCE, PairSide, PairedGain, SamplePair,
};
use crate::combine::normalization::planned_normalization::{PlannedNormalization, SetCoverage};
use crate::combine::normalization::*;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::weighting::Weighting;
use crate::combine::stack_frame::StackFrame;
use crate::frame::admitted::Admitted;
use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_quality::{DropPlanes, FrameQuality};
use crate::frame::frame_stats::{FrameStats, SlotSpread};
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::{FrameWrite, SetKind};
use crate::frame_store::stratified_samples::{SAMPLE_LIMIT, StratifiedSamples};
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::bg_split::BgSplit;
use crate::internals::cfa::make_cfa;
use crate::internals::synthetic::patterns;
use crate::internals::synthetic::sky_field::{GaussianStars, Sky, SkyField};
use crate::internals::test_rng::TestRng;
use crate::math::statistics;
use crate::progress::progress_monitor::ProgressMonitor;
use common::{Size2us, Vec2us};

/// Statistics with one `(median, mad)` per channel and nothing else stated.
fn channel_stats(channels: &[(f32, f32)]) -> Admitted {
    let stats = FrameStats {
        medians: channels.iter().map(|&(median, _)| median).collect(),
        bg: channels
            .iter()
            .map(|&(_, mad)| statistics::internals::mad_to_sigma(mad))
            .map(BgSplit::of_sigma)
            .collect(),
        sky: channels.iter().map(|&(median, _)| median).collect(),
        spread: None,
        quantization_sigma: None,
        electrons_per_unit: None,
    };
    let facts = FrameFacts {
        domain: None,
        row_order: None,
        cfa_type: None,
        saturation_flagged: false,
        conditions: CaptureConditions::default(),
        dark_comparison: None,
    };
    Admitted { stats, facts }
}

fn frame_stats(median: f32, mad: f32) -> Admitted {
    channel_stats(&[(median, mad)])
}

/// Three 5×1 RGB frames covering pixels 1..=3 alone, each channel an exact affine image of the
/// others there, with the source MADs `mads`:
///   ch0: f0 = [2,3,4], f1 = [20,30,40], f2 = [8,9,10]
///   ch1: f0 = [20,30,40], f1 = [2,3,4], f2 = [50,70,90]
///   ch2: f0 = [8,9,10], f1 = [200,300,400], f2 = [30,40,50]
fn affine_rgb_frames(mads: [f32; 3]) -> Vec<StoredFrame> {
    let desc = PlanarDesc::new((5, 1), ChannelLayout::Rgb);
    let coverage = Buffer2::new((5, 1), vec![0.0, 1.0, 1.0, 1.0, 0.0]);
    let channels = [
        [
            vec![1.0, 2.0, 3.0, 4.0, 5.0],
            vec![10.0, 20.0, 30.0, 40.0, 50.0],
            vec![3.0, 5.0, 7.0, 9.0, 11.0],
        ],
        [
            vec![10.0, 20.0, 30.0, 40.0, 50.0],
            vec![1.0, 2.0, 3.0, 4.0, 5.0],
            vec![0.0, 50.0, 70.0, 90.0, 0.0],
        ],
        [
            vec![7.0, 8.0, 9.0, 10.0, 11.0],
            vec![100.0, 200.0, 300.0, 400.0, 500.0],
            vec![20.0, 30.0, 40.0, 50.0, 60.0],
        ],
    ];
    channels
        .into_iter()
        .zip(mads)
        .map(|(channels, mad)| {
            StoredFrame::from_memory(
                LinearImage::from_planar_channels(desc, channels),
                FrameQuality::from_coverage(coverage.clone()),
                channel_stats(&[(0.0, mad); 3]),
            )
        })
        .collect()
}

/// The reference is the frame of the lowest mean noise variance over its channels, in the shared
/// domain. The fixtures' noise is 1.4826 × the stated MAD.
/// - One channel, MADs 2, 0.5, 1: frame 1.
/// - RGB, MADs (1, 1, 5) against (2, 2, 2): mean variances 27/3 = 9 and 4, so frame 1, although
///   frame 0 is quieter in two channels.
/// - Equal noise at different levels: the first.
/// - MADs 2 and 1, the second frame's units 4 of the first's: in the shared domain its σ is 4,
///   above 2, so frame 0; compared raw, frame 1 would win.
#[test]
fn reference_selection_uses_lowest_channel_noise_in_one_domain() {
    let select = |stats: Vec<Admitted>, gains: &[f64]| {
        let frames: Vec<StoredFrame> = stats
            .into_iter()
            .map(|stats| {
                let channels = stats.stats.medians.len();
                StoredFrame::from_memory(
                    LinearImage::from_interleaved(
                        PlanarDesc::new((1, 1), ChannelLayout::try_from(channels).unwrap()),
                        vec![0.0; channels],
                    ),
                    FrameQuality::None,
                    stats,
                )
            })
            .collect();
        let maps: Vec<DomainMap> = gains
            .iter()
            .map(|&gain| DomainMap { gain, offset: 0.0 })
            .collect();
        select_reference_frame(&frames, &maps)
    };
    let single_channel = vec![
        frame_stats(100.0, 2.0),
        frame_stats(100.0, 0.5),
        frame_stats(100.0, 1.0),
    ];
    assert_eq!(select(single_channel, &[1.0; 3]), 1);

    let rgb = vec![
        channel_stats(&[(100.0, 1.0), (100.0, 1.0), (100.0, 5.0)]),
        channel_stats(&[(100.0, 2.0); 3]),
    ];
    assert_eq!(select(rgb, &[1.0; 2]), 1);

    assert_eq!(select(vec![frame_stats(50.0, 3.0)], &[1.0]), 0);
    let equal = vec![
        frame_stats(100.0, 1.5),
        frame_stats(200.0, 1.5),
        frame_stats(300.0, 1.5),
    ];
    assert_eq!(select(equal, &[1.0; 3]), 0);

    let scaled = vec![frame_stats(100.0, 2.0), frame_stats(100.0, 1.0)];
    assert_eq!(select(scaled.clone(), &[1.0, 4.0]), 0);
    assert_eq!(select(scaled, &[1.0, 1.0]), 1);
}

/// `samples`, of noise variance `noise_variance`, as one side of a pair.
fn side(samples: &[f32], noise_variance: f64) -> PairSide<'_> {
    PairSide::new(
        samples,
        noise_variance,
        &mut Vec::new(),
        &CancelToken::never(),
    )
    .unwrap()
}

/// The gain [`SamplePair::photometric_gain`] fits of `frame` onto `reference`.
fn paired_gain(frame: PairSide<'_>, reference: PairSide<'_>) -> PairedGain {
    SamplePair { frame, reference }
        .photometric_gain(&mut Vec::new(), &CancelToken::never())
        .unwrap()
}

/// A noiseless line `2x + 5` with one pair thrown far off it: the window drops that pair and the
/// fit through the rest is the line's slope exactly.
#[test]
fn paired_gain_recovers_scale_after_residual_clipping() {
    let frame: Vec<f32> = (0..101).map(|value| value as f32).collect();
    let mut reference: Vec<f32> = frame.iter().map(|value| value * 2.0 + 5.0).collect();
    reference[50] = 10_000.0;
    let paired = paired_gain(side(&frame, 1.0), side(&reference, 4.0));
    assert_eq!(paired.gain, 2.0);
}

/// The line is exact when every residual lies within the resolution of the median residual, one
/// `f32` step at the reference median 1024, `h = 2⁻¹³`. Frame [0, 1, 2, 3, 4] has median 2 and MAD
/// 1, and reference [1022 + h/2, 1023 + h/2, 1024, 1025, 1026 + d] median 1024 and MAD 1, so the
/// seed gain is 1 and the residuals off `x + 1022` are [h/2, h/2, 0, 0, d]: median h/2, MAD h/2.
/// At d = h the deviations from the median are at most h/2: exact, with no error. At d = 2h the
/// last is 3h/2, past the resolution: no fit, and the gain keeps the seed's error, that of a
/// ratio of two MADs of five, `√(2 · 1.3605 / 5)`.
#[test]
fn an_exact_line_holds_every_residual_to_the_resolution() {
    let h = 2.0f32.powi(-13);
    let frame = [0.0, 1.0, 2.0, 3.0, 4.0];
    let gain = |d: f32| {
        let reference = [
            1022.0 + h / 2.0,
            1023.0 + h / 2.0,
            1024.0,
            1025.0,
            1026.0 + d,
        ];
        paired_gain(side(&frame, 0.0), side(&reference, 0.0))
    };
    assert_eq!(
        gain(h),
        PairedGain {
            gain: 1.0,
            relative_error: 0.0
        }
    );
    assert_eq!(
        gain(2.0 * h),
        PairedGain {
            gain: 1.0,
            relative_error: (2.0 * MAD_SIGMA_VARIANCE / 5.0).sqrt()
        }
    );
}

/// Deming's slope depends on the noise ratio `λ = σ²_ref/σ²_frame`. Frame [8, 9, 10, 11, 12]
/// against reference [5, 9, 10, 12, 15]: the seed is the ratio of the MADs, 2/1; the residuals off
/// it, −1, 1, 0, 0, 1, have a MAD of 1, whose 4σ window admits all five, and the window on the
/// fitted gain admits them again. About the means 10 and 10.2 they carry `S_xx = 10`, `S_yy = 54.8`
/// and `S_xy = 23`, so the slope `(S_yy − λS_xx + √((S_yy − λS_xx)² + 4λS_xy²))/(2S_xy)` is
/// 2.369802 at λ = 1 and 2.347453 at λ = 4 — between ordinary least squares' 2.3 and the reverse
/// fit's 2.383 — each rounded once to f32. Only the ratio counts, and no noise stated on a side
/// means λ = 1.
#[test]
fn deming_gain_weighs_each_sides_noise() {
    let frame = [8.0, 9.0, 10.0, 11.0, 12.0];
    let reference = [5.0, 9.0, 10.0, 12.0, 15.0];
    let gain = |frame_noise, reference_noise| {
        paired_gain(side(&frame, frame_noise), side(&reference, reference_noise)).gain
    };
    assert_eq!(gain(1.0, 1.0), 2.369_802_2);
    assert_eq!(gain(1.0, 4.0), 2.347_452_9);
    assert_eq!(gain(4.0, 4.0), gain(1.0, 1.0));
    assert_eq!(gain(0.0, 0.0), gain(1.0, 1.0));
}

/// The Deming gain's stated error is the scatter of the gains it gives: 400 draws of 2000 pairs of
/// a field `x = 0.1 + 0.5·u³`, `u` uniform — mostly sky, with a tail of stars — read by the frame
/// with noise σ 0.01 and by the reference as `1.6·x + 0.02` with noise σ 0.015. The relative
/// standard deviation of the 400 gains and the mean stated error agree within 10%: the scatter of
/// a standard deviation over 400 draws is 1/√798 ≈ 3.5% of it, so 10% is about three of those.
#[test]
fn the_deming_error_is_the_scatter_of_the_gains() {
    let mut rng = TestRng::new(0xDE41);
    let draws = 400;
    let mut gains = Vec::with_capacity(draws);
    let mut stated = 0.0;
    for _ in 0..draws {
        let field: Vec<f32> = (0..2000)
            .map(|_| 0.1 + 0.5 * rng.next_f32().powi(3))
            .collect();
        let frame: Vec<f32> = field
            .iter()
            .map(|&x| x + 0.01 * rng.next_gaussian_f32())
            .collect();
        let reference: Vec<f32> = field
            .iter()
            .map(|&x| 1.6 * x + 0.02 + 0.015 * rng.next_gaussian_f32())
            .collect();
        let paired = paired_gain(
            side(&frame, 0.01f64.powi(2)),
            side(&reference, 0.015f64.powi(2)),
        );
        gains.push(f64::from(paired.gain));
        stated += paired.relative_error;
    }
    let mean = gains.iter().sum::<f64>() / draws as f64;
    let scatter =
        (gains.iter().map(|gain| (gain - mean).powi(2)).sum::<f64>() / (draws - 1) as f64).sqrt()
            / mean;
    let stated = stated / draws as f64;
    assert!(
        (mean - 1.6).abs() < 3.0 * scatter * mean / (draws as f64).sqrt(),
        "{mean}"
    );
    assert!(
        (stated / scatter - 1.0).abs() < 0.1,
        "stated {stated}, measured {scatter}"
    );
}

/// Pairs that fall where the frame rises carry no positive covariance, and the Deming fit cannot
/// move the gain: it stays at the seed. Frame [8, 9, 10, 11, 12] against reference
/// [15, 12, 10, 9, 5]: too few pairs to seed from stars, so the seed is the ratio of the MADs, 2/1.
/// Off it the residuals are 9, 4, 0, −3, −9, MAD 4, so the 4σ window admits all five, whose
/// covariance is negative. The old fallback replaced the seed with unit gain.
#[test]
fn a_fit_without_positive_covariance_keeps_the_seed() {
    let frame = [8.0, 9.0, 10.0, 11.0, 12.0];
    let reference = [15.0, 12.0, 10.0, 9.0, 5.0];
    let paired = paired_gain(side(&frame, 1.0), side(&reference, 1.0));
    assert_eq!(paired.gain, 2.0);
}

/// A multiplicative normalization divides by each frame's median, so a frame at a median of 0 is
/// refused, by index and channel, rather than combined at unit gain. Uniform frames tie for the
/// reference, which frame 0 takes.
#[test]
fn a_multiplicative_norm_refuses_a_non_positive_median() {
    let cache = FrameCache::from_images(
        [1.0, 0.0]
            .map(|level| {
                LinearImage::from_interleaved(
                    PlanarDesc::new((16, 1), ChannelLayout::Mono),
                    vec![level; 16],
                )
            })
            .to_vec(),
        Normalization::None,
    );
    let result = FrameNorm::measure(
        SlotPlanes {
            frames: &cache.frames,
            desc: cache.core.desc,
            slots: cache.slots,
            cancel: &CancelToken::never(),
        },
        Normalization::Multiplicative,
    );
    assert!(matches!(
        result,
        Err(StackError::NonPositiveMedian {
            idx: 1,
            slot: 0,
            median: 0.0
        })
    ));
}

/// The norms of whole frames, measured as the cache measures them, as `(gain, offset)` per frame
/// and channel.
fn norms(images: Vec<LinearImage>, normalization: Normalization) -> Vec<(f32, f32)> {
    let cache = FrameCache::from_images(images, Normalization::None);
    FrameNorm::measure(
        SlotPlanes {
            frames: &cache.frames,
            desc: cache.core.desc,
            slots: cache.slots,
            cancel: &CancelToken::never(),
        },
        normalization,
    )
    .unwrap()
    .unwrap()
    .iter()
    .flat_map(|norm| norm.slots.iter().map(|slot| (slot.gain, slot.offset)))
    .collect()
}

/// Frames whose relation is exact normalize exactly, by hand.
///
/// - Uniform frames have no spread: the seed falls back to unit gain and every residual is the
///   same, so the line through the medians is already exact. Global takes gain 1 and the levels'
///   difference as the offset; multiplicative takes the levels' ratio. Identical frames are the
///   identity under both.
/// - Two 4×4 ramps 100 + i and 200 + i have the same MAD, 4, so the seed is 1 and every residual
///   is 0: gain 1, offset −100.
/// - Ramps 80 + i/2, 198 + i/16 and 140 + i/8 over 100 pixels are dyadic, so every step is exact.
///   A ramp has no white noise, so the three tie for the reference and frame 0 takes it. Their
///   MADs are 25 steps, 12.5, 1.5625 and 3.125, so the seeds 12.5/1.5625 = 8 and 12.5/3.125 = 4
///   leave every residual 0. The offsets put the medians 104.75, 201.09375 and 146.1875 together:
///   104.75 − 8·201.09375 = −1504, and 104.75 − 4·146.1875 = −480.
/// - Multiplicative never shifts, whatever the frames.
#[test]
fn frames_related_exactly_normalize_exactly() {
    let uniform = |values: &[f32]| {
        values
            .iter()
            .map(|&value| {
                LinearImage::from_interleaved(
                    PlanarDesc::new((16, 1), ChannelLayout::Mono),
                    vec![value; 16],
                )
            })
            .collect::<Vec<_>>()
    };
    let ramp = |count: usize, start: f32, step: f32| {
        LinearImage::from_interleaved(
            PlanarDesc::new((count, 1), ChannelLayout::Mono),
            (0..count).map(|i| start + i as f32 * step).collect(),
        )
    };
    for normalization in [Normalization::Global, Normalization::Multiplicative] {
        assert_eq!(
            norms(uniform(&[5.0; 3]), normalization),
            [(1.0, 0.0); 3],
            "{normalization:?}"
        );
    }
    assert_eq!(
        norms(uniform(&[100.0, 150.0]), Normalization::Global),
        [(1.0, 0.0), (1.0, -50.0)]
    );
    assert_eq!(
        norms(uniform(&[100.0, 200.0]), Normalization::Multiplicative),
        [(1.0, 0.0), (0.5, 0.0)]
    );
    assert_eq!(
        norms(
            vec![ramp(16, 100.0, 1.0), ramp(16, 200.0, 1.0)],
            Normalization::Global
        ),
        [(1.0, 0.0), (1.0, -100.0)]
    );
    let dyadic = || {
        vec![
            ramp(100, 80.0, 0.5),
            ramp(100, 198.0, 0.0625),
            ramp(100, 140.0, 0.125),
        ]
    };
    assert_eq!(
        norms(dyadic(), Normalization::Global),
        [(1.0, 0.0), (8.0, -1504.0), (4.0, -480.0)]
    );
    assert!(
        norms(dyadic(), Normalization::Multiplicative)
            .iter()
            .all(|&(_, offset)| offset == 0.0)
    );
}

/// After normalization a mean stack sits at the reference frame's level, per channel; without it,
/// between the frames. Each normalized frame equals the reference exactly here — `150·(2/3)`
/// rounds to 100 in f32 — so the mean is the reference exactly.
#[test]
fn stacked_frames_land_on_the_reference_level() {
    let rgb = |values: [[f32; 3]; 2]| {
        values
            .iter()
            .map(|rgb| {
                LinearImage::from_planar_channels(
                    PlanarDesc::new((4, 1), ChannelLayout::Rgb),
                    rgb.map(|value| vec![value; 4]),
                )
            })
            .collect::<Vec<_>>()
    };
    let reference = [100.0, 200.0, 300.0];
    for (normalization, frames, expected) in [
        (
            Normalization::Global,
            rgb([reference, [120.0, 180.0, 350.0]]),
            reference,
        ),
        (
            Normalization::Multiplicative,
            rgb([reference, [150.0, 100.0, 600.0]]),
            reference,
        ),
        (
            Normalization::None,
            rgb([reference, [200.0, 300.0, 400.0]]),
            [150.0, 250.0, 350.0],
        ),
    ] {
        let product = (StackConfig {
            combine: Combine::mean(),
            normalization,
            weighting: Weighting::Equal,
            ..StackConfig::light()
        })
        .stack_images(
            frames.into_iter().map(StackFrame::from).collect(),
            ProgressMonitor::unattended(),
        )
        .unwrap();
        for (channel, &level) in expected.iter().enumerate() {
            assert_eq!(
                product.image.channel(channel).pixels(),
                &[level; 4],
                "{normalization:?} channel {channel}"
            );
        }
    }
}

/// The common domain is the coverage floor's intersection, not "coverage at all". A pixel a frame
/// barely touched is warp border fill, and measuring the photometric scale on it would compare fill
/// against data — even though the interpolation there was perfectly confident, which is why the
/// separate `confidence > 0` intersection this replaced could never have excluded it.
#[test]
fn common_domain_excludes_pixels_covered_only_by_border_fill() {
    let desc = PlanarDesc::new((4, 1), ChannelLayout::Mono);
    let coverage = Buffer2::new((4, 1), vec![1.0, 0.5, 1e-4, 0.0]);
    let confidence = Buffer2::new((4, 1), vec![1.0, 2.0, 4.0, 0.0]);
    let image = LinearImage::from_interleaved(desc, vec![0.5; 4]);
    let frame = StoredFrame::from_memory(
        image,
        FrameQuality::Planes {
            coverage,
            confidence,
        },
        frame_stats(0.5, 0.1),
    );

    let domain = CommonDomain::build(
        ChannelCoverage {
            frames: &[&frame],
            channel: 0,
            pixel_count: desc.pixel_count(),
        },
        &CancelToken::never(),
    )
    .expect("two pixels clear the floor");
    // Full support and half support are data; 1e-4 is under the 1e-3 floor, and 0.0 is the border.
    assert!(domain.valid.get(0));
    assert!(domain.valid.get(1));
    assert!(
        !domain.valid.get(2),
        "border fill entered the common domain"
    );
    assert!(!domain.valid.get(3));
    assert_eq!(domain.sample_count, 2);

    // A frame gathered in each channel by its own drops narrows each channel's domain by its own:
    // beside a warped frame covering pixels 0 to 2, drops in pixels {0, 1, 3}, {1, 2, 3} and
    // {0, 2, 3} leave {0, 1}, {1, 2} and {0, 2}.
    let rgb = PlanarDesc::new((4, 1), ChannelLayout::Rgb);
    let warped = StoredFrame::from_memory(
        LinearImage::from_interleaved(rgb, vec![0.5; 12]),
        FrameQuality::from_coverage(Buffer2::new((4, 1), vec![1.0, 1.0, 1.0, 0.0])),
        channel_stats(&[(0.5, 0.1); 3]),
    );
    let drops = |weights: [f32; 4]| DropPlanes {
        weight: Buffer2::new((4, 1), weights.to_vec()),
        confidence: Buffer2::new((4, 1), weights.to_vec()),
    };
    let drizzled = StoredFrame::from_memory(
        LinearImage::from_interleaved(rgb, vec![0.5; 12]),
        FrameQuality::Drizzled {
            drops: [
                drops([1.0, 1.0, 0.0, 1.0]),
                drops([0.0, 1.0, 1.0, 1.0]),
                drops([1.0, 0.0, 1.0, 1.0]),
            ]
            .into_iter()
            .collect(),
        },
        channel_stats(&[(0.5, 0.1); 3]),
    );
    for (channel, expected) in [
        [true, true, false, false],
        [false, true, true, false],
        [true, false, true, false],
    ]
    .into_iter()
    .enumerate()
    {
        let domain = CommonDomain::build(
            ChannelCoverage {
                frames: &[&warped, &drizzled],
                channel,
                pixel_count: 4,
            },
            &CancelToken::never(),
        )
        .unwrap();
        assert_eq!(
            (0..4)
                .map(|pixel| domain.valid.get(pixel))
                .collect::<Vec<_>>(),
            expected,
            "channel {channel}"
        );
    }
}

/// Global norms are fitted against whichever frame was selected as the reference, not against
/// frame 0 and rescaled afterwards.
///
/// The same three frames as the test below, but with the source noise that picks the reference
/// arranged so frame 2 wins it. Every gain and offset must then be the one that carries a frame
/// onto *frame 2*, and frame 2 itself must come back exactly identity.
///
/// The three frames are exact affine transforms of each other, which is deliberate: on data this
/// clean, fitting `a→c` and chaining `a→b→c` agree, so this pins the indexing rather than the
/// numerics. What the direct fit buys shows up only where the errors-in-variables fit clips
/// residuals and weights each side by its own noise, and that is a real-data check.
#[test]
fn global_norms_are_fitted_against_the_selected_reference() {
    // Per channel the three frames are affine images of frame 2 over the common domain:
    //   ch0: f0·1 + 6, f1·0.1 + 6;  ch1: f0·10 + 0, f1·100 + 0;  ch2: f0·5 + 5, f1·0.5 + 5.
    // Frame 2 is the least noisy, so `select_reference_frame` picks it.
    let desc = PlanarDesc::new((5, 1), ChannelLayout::Rgb);
    let frames = affine_rgb_frames([3.0, 2.0, 1.0]);
    assert_eq!(
        select_reference_frame(&frames, &[DomainMap::IDENTITY; 3]),
        2,
        "the fixture must not select frame 0, or it proves nothing"
    );

    let norms = FrameNorm::measure(
        SlotPlanes {
            frames: &frames,
            desc,
            slots: Slots::of_frames(&frames, desc),
            cancel: &CancelToken::never(),
        },
        Normalization::Global,
    )
    .unwrap()
    .expect("global normalization returns parameters");

    let expected = [
        [(1.0, 6.0), (10.0, 0.0), (5.0, 5.0)],
        [(0.1, 6.0), (100.0, 0.0), (0.5, 5.0)],
        [(1.0, 0.0), (1.0, 0.0), (1.0, 0.0)],
    ];
    for (frame_idx, frame) in norms.iter().enumerate() {
        for (channel, &(gain, offset)) in expected[frame_idx].iter().enumerate() {
            assert_eq!(
                frame.slots[channel].gain, gain,
                "frame {frame_idx} channel {channel} gain"
            );
            assert_eq!(
                frame.slots[channel].offset, offset,
                "frame {frame_idx} channel {channel} offset"
            );
        }
    }
}

/// Multiplicative and global norms over the common domain, per frame and channel in order, and a
/// cancel honoured. The domain is pixels 1..=3, where the channels' medians are frame 0: 3, 30, 7;
/// frame 1: 30, 3, 70; frame 2: 9, 300, 40. Every frame carries the same source noise, so frame 0
/// is the reference. Each frame is an exact affine image of it, so the global fit has nothing to
/// window and returns the ratio of spreads, and its offset puts the medians together.
#[test]
fn common_domain_norms_preserve_pair_order_and_honor_cancellation() {
    let desc = PlanarDesc::new((5, 1), ChannelLayout::Rgb);
    let frames = affine_rgb_frames([1.0; 3]);
    let norms = |normalization| {
        FrameNorm::measure(
            SlotPlanes {
                frames: &frames,
                desc,
                slots: Slots::of_frames(&frames, desc),
                cancel: &CancelToken::never(),
            },
            normalization,
        )
        .unwrap()
        .unwrap()
    };

    let expected_gains = [
        [1.0, 1.0, 1.0],
        [3.0 / 30.0, 30.0 / 3.0, 7.0 / 70.0],
        [3.0f32 / 9.0, 30.0 / 300.0, 7.0 / 40.0],
    ];
    for (frame_idx, frame) in norms(Normalization::Multiplicative).iter().enumerate() {
        for (channel, &gain) in expected_gains[frame_idx].iter().enumerate() {
            assert_eq!(
                frame.slots[channel],
                SlotNorm { gain, offset: 0.0 },
                "frame {frame_idx} channel {channel}"
            );
        }
    }

    let expected_norms = [
        [(1.0, 0.0), (1.0, 0.0), (1.0, 0.0)],
        [(0.1, 0.0), (10.0, 0.0), (0.1, 0.0)],
        [(1.0, -6.0), (0.1, 0.0), (0.2, -1.0)],
    ];
    for (frame_idx, frame) in norms(Normalization::Global).iter().enumerate() {
        for (channel, &(gain, offset)) in expected_norms[frame_idx].iter().enumerate() {
            assert_eq!(
                frame.slots[channel],
                SlotNorm { gain, offset },
                "frame {frame_idx} channel {channel}"
            );
        }
    }

    let cancel = CancelToken::new();
    cancel.cancel();
    let error = FrameNorm::measure(
        SlotPlanes {
            frames: &frames,
            desc,
            slots: Slots::of_frames(&frames, desc),
            cancel: &cancel,
        },
        Normalization::Global,
    )
    .unwrap_err();
    assert!(matches!(error, StackError::Cancelled));
}

/// A star field seen through three frames of known gain, offset and noise: `x_k = (truth −
/// o_k)/g_k + n_k`. Frame 0 is the least noisy and so the reference, and the gain that carries
/// frame `k` onto it is `g_k`.
///
/// The frames' sky noise is unrelated to their gain — 0.006 at gain 0.8, 0.004 at 1.25 — so the
/// ratio of sky spreads, the obvious seed for unregistered frames, misses every gain by far. The
/// fit must find each within its own noise: the Deming slope's standard error is `√((σ²_ref +
/// g²σ²_k) / S_xx)`, with `S_xx` the frame's spread over the paired pixels, and 5 of those bound
/// it.
///
/// The same set with one pixel of frame 2 declared blank measures over the common domain instead
/// of every pixel. That moves which pixels are measured by one, not which estimator runs, so the
/// gains agree with the unblanked ones to within one standard error.
#[test]
fn global_gains_are_recovered_from_a_star_field_and_one_blank_pixel_does_not_move_them() {
    let size = Size2us::new(256, 256);
    let mut rng = TestRng::new(0x9a17);
    let stars: Vec<(Vec2, f32)> = (0..150)
        .map(|_| {
            let center = Vec2::new(
                8.0 + rng.next_f32() * (size.width - 16) as f32,
                8.0 + rng.next_f32() * (size.height - 16) as f32,
            );
            (center, 0.02 + rng.next_f32() * 0.78)
        })
        .collect();
    let sky = Sky {
        level: 0.1,
        noise: None,
        clamp: false,
    };
    let truth = SkyField::render(
        size,
        sky,
        GaussianStars {
            sigma: 1.5,
            peaks: &stars,
        },
    )
    .pixels;
    let desc = PlanarDesc::new((size.width, size.height), ChannelLayout::Mono);
    let settings = [
        (1.0f32, 0.0f32, 0.002f32),
        (0.8, 0.02, 0.006),
        (1.25, -0.03, 0.004),
    ];
    let images: Vec<LinearImage> = settings
        .iter()
        .enumerate()
        .map(|(k, &(gain, offset, noise))| {
            let mut pixels: Vec<f32> = truth
                .pixels()
                .iter()
                .map(|&t| (t - offset) / gain)
                .collect();
            patterns::add_gaussian_noise(&mut pixels, noise, 100 + k as u64);
            LinearImage::from_interleaved(desc, pixels)
        })
        .collect();
    let stored = |blank: Option<usize>| -> Vec<StoredFrame> {
        images
            .iter()
            .enumerate()
            .map(|(k, image)| {
                let quality = match blank {
                    Some(pixel) if k == 2 => {
                        let mut coverage = vec![1.0; size.pixel_count()];
                        coverage[pixel] = 0.0;
                        FrameQuality::from_coverage(Buffer2::new(size, coverage))
                    }
                    _ => FrameQuality::None,
                };
                StoredFrame::from_memory(image.clone(), quality, Admitted::measure(image))
            })
            .collect()
    };
    let gains = |frames: &[StoredFrame]| -> Vec<f32> {
        FrameNorm::measure(
            SlotPlanes {
                frames,
                desc,
                slots: Slots::of_frames(frames, desc),
                cancel: &CancelToken::never(),
            },
            Normalization::Global,
        )
        .unwrap()
        .unwrap()
        .iter()
        .map(|norm| norm.slots[0].gain)
        .collect()
    };

    let mean = truth.pixels().iter().map(|&t| f64::from(t)).sum::<f64>() / truth.len() as f64;
    let truth_spread: f64 = truth
        .pixels()
        .iter()
        .map(|&t| (f64::from(t) - mean).powi(2))
        .sum();
    let standard_error = |k: usize| {
        let (gain, _, noise) = settings[k];
        let (gain, noise, reference_noise) = (f64::from(gain), f64::from(noise), 0.002f64);
        let frame_spread = truth_spread / (gain * gain);
        ((reference_noise.powi(2) + gain * gain * noise.powi(2)) / frame_spread).sqrt()
    };

    let full = gains(&stored(None));
    let blanked = gains(&stored(Some(100 * size.width + 100)));
    assert_eq!(
        (full[0], blanked[0]),
        (1.0, 1.0),
        "frame 0 is the reference"
    );
    for k in 1..3 {
        let expected = f64::from(settings[k].0);
        let error = standard_error(k);
        assert!(
            (f64::from(full[k]) - expected).abs() <= 5.0 * error,
            "frame {k}: gain {} vs {expected} (σ {error:e})",
            full[k]
        );
        assert!(
            (f64::from(blanked[k]) - f64::from(full[k])).abs() <= error,
            "frame {k}: blank moved the gain from {} to {} (σ {error:e})",
            full[k],
            blanked[k]
        );
    }
}

/// Past `SAMPLE_LIMIT` measured pixels the samples are those of rank `⌊k·n/m⌋`. Over 200 000
/// pixels with no domain the k-th of 65 536 is `⌊k·3.0518⌋`: 0, 3, 6, 9, …, and 199 996 last. Over a
/// domain of the 100 000 even pixels the rank is `⌊k·1.5259⌋` — 0, 1, 3, 4 — and the pixel twice
/// that: 0, 2, 6, 8, …, 199 996 last. Ten pixels are under the limit: every one.
///
/// A 4096×64 RGGB mosaic, sampled as one plane, would take every 4th pixel: even columns only, red
/// and the green of the red rows. Each color is drawn by rank among its own photosites instead:
/// red's and blue's 65 536 are all taken, and every second of green's 131 072, 1024 a row, half of
/// them on each green phase.
#[test]
fn samples_spread_evenly_by_rank_past_the_limit() {
    let cancel = CancelToken::never();
    let pixel_count = 200_000;
    let check = |idxs: Vec<usize>, first: [usize; 4], last: usize| {
        assert_eq!(idxs.len(), SAMPLE_LIMIT);
        assert_eq!(idxs[..4], first);
        assert_eq!(idxs[idxs.len() - 1], last);
        assert!(idxs.is_sorted() && idxs.windows(2).all(|pair| pair[0] < pair[1]));
    };
    let every = |size: Size2us| {
        StratifiedSamples::new(size, None, 0)
            .idxs()
            .collect::<Vec<_>>()
    };
    check(every(Size2us::new(pixel_count, 1)), [0, 3, 6, 9], 199_996);

    let even = Buffer2::from_fn((pixel_count, 1), |x, _| if x % 2 == 0 { 1.0 } else { 0.0 });
    let frame = StoredFrame::from_memory(
        LinearImage::from_interleaved(
            PlanarDesc::new((pixel_count, 1), ChannelLayout::Mono),
            vec![0.5; pixel_count],
        ),
        FrameQuality::from_coverage(even),
        frame_stats(0.5, 0.1),
    );
    let domain = CommonDomain::build(
        ChannelCoverage {
            frames: &[&frame],
            channel: 0,
            pixel_count,
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(domain.sample_count, 100_000);
    check(
        domain.stratified_idxs(&cancel).unwrap(),
        [0, 2, 6, 8],
        199_996,
    );

    assert_eq!(every(Size2us::new(10, 1)), (0..10).collect::<Vec<_>>());

    let size = Size2us::new(4096, 64);
    let rggb = Mosaic::Bayer(BayerPattern::Rggb);
    for (slot, expected) in [[65_536, 0, 0, 0], [0, 32_768, 32_768, 0], [0, 0, 0, 65_536]]
        .into_iter()
        .enumerate()
    {
        let mut phases = [0; 4];
        for idx in StratifiedSamples::new(size, Some(rggb), slot).idxs() {
            phases[(idx / 4096 % 2) * 2 + idx % 2] += 1;
        }
        assert_eq!(
            phases, expected,
            "slot {slot}: R, Gr, Gb, B photosites sampled"
        );
    }
}

/// A warped frame's noise variance at the samples is its source σ² scaled by the mean inverse
/// confidence there: interpolation that averaged pixels left less noise in each. Confidences 1,
/// 1/2, 1/4 and 1 average an inverse of (1 + 2 + 4 + 1)/4 = 2, every step a power of two, so the
/// variance is exactly 2σ². A frame with no confidence plane keeps σ².
#[test]
fn noise_variance_scales_by_the_mean_inverse_confidence() {
    let cancel = CancelToken::never();
    let image =
        LinearImage::from_interleaved(PlanarDesc::new((4, 1), ChannelLayout::Mono), vec![0.5; 4]);
    let stats = frame_stats(0.5, 0.25);
    let sigma = f64::from(statistics::internals::mad_to_sigma(0.25f32));
    let warped = StoredFrame::from_memory(
        image.clone(),
        FrameQuality::Planes {
            coverage: Buffer2::new((4, 1), vec![1.0; 4]),
            confidence: Buffer2::new((4, 1), vec![1.0, 0.5, 0.25, 1.0]),
        },
        stats.clone(),
    );
    let unwarped = StoredFrame::from_memory(image, FrameQuality::None, stats);
    let idxs = [0, 1, 2, 3];
    let mono = Slots::new(None, 1);
    assert_eq!(
        SlotPlanes {
            frames: slice::from_ref(&warped),
            desc: PlanarDesc::new((4, 1), ChannelLayout::try_from(mono.count()).unwrap()),
            slots: mono,
            cancel: &cancel
        }
        .noise_variance(0, 0, &idxs)
        .unwrap(),
        2.0 * sigma * sigma
    );
    assert_eq!(
        SlotPlanes {
            frames: slice::from_ref(&unwarped),
            desc: PlanarDesc::new((4, 1), ChannelLayout::try_from(mono.count()).unwrap()),
            slots: mono,
            cancel: &cancel
        }
        .noise_variance(0, 0, &idxs)
        .unwrap(),
        sigma * sigma
    );

    // A mosaic drizzled per color divides each slot by its own channel's Kish sizes: 1, 1/2, 1/4
    // and 1 in green again average an inverse of 2, the red's 1/2 everywhere an inverse of 2 too,
    // and the blue's 1 everywhere keeps σ². The color planes are the frame's channels, so slot
    // `c` is channel `c`.
    let rgb = PlanarDesc::new((4, 1), ChannelLayout::Rgb);
    let drops = |confidence: [f32; 4]| DropPlanes {
        weight: Buffer2::new((4, 1), vec![1.0; 4]),
        confidence: Buffer2::new((4, 1), confidence.to_vec()),
    };
    let drizzled = StoredFrame::from_memory(
        LinearImage::from_interleaved(rgb, vec![0.5; 12]),
        FrameQuality::Drizzled {
            drops: [
                drops([0.5; 4]),
                drops([1.0, 0.5, 0.25, 1.0]),
                drops([1.0; 4]),
            ]
            .into_iter()
            .collect(),
        },
        channel_stats(&[(0.5, 0.25); 3]),
    );
    let colors = Slots::new(None, 3);
    for (slot, expected) in [(0, 2.0), (1, 2.0), (2, 1.0)] {
        assert_eq!(
            SlotPlanes {
                frames: slice::from_ref(&drizzled),
                desc: PlanarDesc::new((4, 1), ChannelLayout::try_from(colors.count()).unwrap()),
                slots: colors,
                cancel: &cancel
            }
            .noise_variance(0, slot, &idxs)
            .unwrap(),
            expected * sigma * sigma,
            "slot {slot}"
        );
    }
}

/// Twilight flats drift in color: three RGGB flats whose red, green and blue scale by (1, 1, 1),
/// (2, 1, 4) and (1/2, 2, 2) over one 16×16 pattern of levels `1/4 + (37i mod 64)/1024`. Every
/// level and scale is dyadic, so each frame's color medians are the pattern's times its scales,
/// exactly. Frame 0 has the lowest noise in every color and is the reference; a multiplicative
/// norm per color is the ratio of scales: (1/2, 1, 1/4) and (2, 1/2, 1/2). One norm per frame
/// would leave two colors of every frame off the reference, which a σ-clip of the flat preset
/// reads as outliers. Normalized per color, the three frames are the reference to the bit: every
/// pixel keeps all three, a survivor weight of 3, and the master is frame 0.
#[test]
fn twilight_flats_normalize_per_color() {
    let size = Size2us::new(16, 16);
    let cfa = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let pattern: Vec<f32> = (0..size.pixel_count())
        .map(|idx| 0.25 + (37 * idx % 64) as f32 / 1024.0)
        .collect();
    let scales = [[1.0, 1.0, 1.0], [2.0, 1.0, 4.0], [0.5, 2.0, 2.0]];
    let images: Vec<CfaImage> = scales
        .iter()
        .map(|scale: &[f32; 3]| {
            let pixels = pattern
                .iter()
                .enumerate()
                .map(|(idx, &level)| {
                    let color = cfa.slot_at(Vec2us::new(idx % 16, idx / 16));
                    level * scale[color]
                })
                .collect();
            make_cfa(size, pixels, cfa)
        })
        .collect();
    let reference = images[0].data().pixels().to_vec();
    let cache = FrameCache::from_images(images, Normalization::Multiplicative);
    let expected = [[1.0, 1.0, 1.0], [0.5, 1.0, 0.25], [2.0, 0.5, 0.5]];
    let norms = cache.frame_norms.as_ref().expect("a fitted normalization");
    for (frame, gains) in expected.iter().enumerate() {
        let slots: Vec<SlotNorm> = gains
            .iter()
            .map(|&gain| SlotNorm { gain, offset: 0.0 })
            .collect();
        assert_eq!(norms[frame].slots.as_slice(), slots, "frame {frame}");
    }

    let product = StackConfig::flat().run_stacking(&cache).unwrap();
    assert_eq!(product.image.channel(0).pixels(), reference);
    let weight = product
        .weight
        .expect("the flat preset keeps the weight plane");
    assert!(
        weight
            .channel(0)
            .pixels()
            .iter()
            .all(|&weight| weight == 3.0),
        "a frame was rejected"
    );
}

/// A frame of no quality written to disk gathers each slot's normalization samples as it goes, so
/// a global normalization reads none of its planes again; the samples are its plane at the
/// stratified indices, and the norms the resident frames give, bit for bit. Four frames of
/// independent noise at different gains and offsets, as RGB and as an RGGB mosaic, each 300×240,
/// past the sample limit for every slot of the RGB frames, and under it for the mosaic's colors.
#[test]
fn spilled_unwarped_frames_normalize_from_their_stored_samples() {
    let size = Size2us::new(300, 240);
    let rggb = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let pixels = |frame: usize, channel: usize| -> Vec<f32> {
        let mut rng = TestRng::new(frame as u64 * 3 + channel as u64);
        let gain = 1.0 + 0.25 * frame as f32;
        (0..size.pixel_count())
            .map(|idx| {
                let level = 0.2 + 0.1 * channel as f32 + (idx % 300) as f32 / 3000.0;
                level * gain + 0.01 * frame as f32 + 0.01 * (rng.next_f32() - 0.5)
            })
            .collect()
    };
    let scratch = TempDir::new("normalization_stored_samples");
    let run_scratch = RunScratch::create(&scratch.join("run")).unwrap();
    let check = |frames: Vec<StoredFrame>, spilled: Vec<StoredFrame>, desc, mosaic| {
        let slots = Slots::of_frames(&frames, desc);
        for frame in &spilled {
            let samples = frame
                .samples
                .as_ref()
                .expect("a frame of no quality on disk");
            assert_eq!(samples.len(), slots.count());
            let mut expected = Vec::new();
            for (slot, stored) in samples.iter().enumerate() {
                StratifiedSamples::new(desc.size(), mosaic, slot).gather(
                    frame.channels[slots.channel(slot)].chunk(0..size.pixel_count()),
                    &mut expected,
                );
                assert_eq!(stored.chunk(0..stored.samples()), expected, "slot {slot}");
            }
        }
        let norms = |frames: &[StoredFrame]| {
            FrameNorm::measure(
                SlotPlanes {
                    frames,
                    desc,
                    slots,
                    cancel: &CancelToken::never(),
                },
                Normalization::Global,
            )
            .unwrap()
            .unwrap()
            .iter()
            .flat_map(|norm| {
                norm.slots
                    .iter()
                    .map(|slot| (slot.gain.to_bits(), slot.offset.to_bits()))
            })
            .collect::<Vec<_>>()
        };
        assert_eq!(norms(&frames), norms(&spilled));
    };

    let rgb = |frame| {
        LinearImage::from_planar_channels(
            PlanarDesc::new(size, ChannelLayout::Rgb),
            (0..3).map(|channel| pixels(frame, channel)),
        )
    };
    let store = |image: &LinearImage| {
        FrameWrite {
            image,
            quality: FrameQuality::None,
            set: SetKind::Unwarped,
        }
        .write(&run_scratch)
        .unwrap()
        .into_frame(Admitted::measure(image))
    };
    check(
        (0..4)
            .map(|frame| StackFrame::from(rgb(frame)))
            .map(|frame| StoredFrame::from_memory(frame.image, frame.quality, frame.admitted))
            .collect(),
        (0..4).map(|frame| store(&rgb(frame))).collect(),
        PlanarDesc::new(size, ChannelLayout::Rgb),
        None,
    );

    let mosaic = |frame| make_cfa(size, pixels(frame, 0), rggb);
    check(
        (0..4)
            .map(|frame| {
                let image = mosaic(frame);
                let stats = Admitted::measure(&image);
                StoredFrame::from_memory(image, FrameQuality::None, stats)
            })
            .collect(),
        (0..4)
            .map(|frame| {
                let image = mosaic(frame);
                FrameWrite {
                    image: &image,
                    quality: FrameQuality::None,
                    set: SetKind::Unwarped,
                }
                .write(&run_scratch)
                .unwrap()
                .into_frame(Admitted::measure(&image))
            })
            .collect(),
        PlanarDesc::new(size, ChannelLayout::Mono),
        rggb.mosaic(),
    );
}

/// Frames gathered in each channel by their own drops are normalized pair by pair: each against
/// the reference over the pixels both reached in that channel. Three 8×1 RGB frames, the
/// reference (frame 2, the quietest) reaching pixels 1 to 6 in every channel at `r = (x + 1)(c + 1)`,
/// frame 0 at `2r + 1` and frame 1 at `r/2 − 1/4`. In channels 0 and 2 frame 0 reaches pixels 0
/// to 3 and frame 1 pixels 4 to 7, in channel 1 the other way round: no pixel is reached by all
/// three in any channel, which a common domain would refuse, while every pair shares three.
///
/// Global fits each pair's exact line: gain `1/a` and offset `−b/a` for a frame at `a·r + b`, so
/// (0.5, −0.5) and (2, 0.5). Multiplicative divides the pair's medians, the middle pixel of each
/// three: `r_mid / (a·r_mid + b)`. Each frame's spread is its noise and its source median 0, so no
/// frame has a scale or a level of its own and the pairs stand alone.
#[test]
fn per_channel_frames_are_normalized_pair_by_pair() {
    let desc = PlanarDesc::new((8, 1), ChannelLayout::Rgb);
    let reference = |c: usize, x: usize| ((x + 1) * (c + 1)) as f32;
    let affine = [(2.0f32, 1.0f32), (0.5, -0.25)];
    let reaches = |frame: usize, channel: usize| -> std::ops::Range<usize> {
        match (frame, channel % 2) {
            (2, _) => 1..7,
            (0, 0) | (1, 1) => 0..4,
            _ => 4..8,
        }
    };
    let frames: Vec<StoredFrame> = (0..3)
        .map(|frame| {
            let value = |c: usize, x: usize| match frame {
                2 => reference(c, x),
                _ => affine[frame].0 * reference(c, x) + affine[frame].1,
            };
            let planes: Vec<Vec<f32>> = (0..3)
                .map(|c| {
                    (0..8)
                        .map(|x| {
                            if reaches(frame, c).contains(&x) {
                                value(c, x)
                            } else {
                                0.0
                            }
                        })
                        .collect()
                })
                .collect();
            let drops = (0..3)
                .map(|c| {
                    let reached = Buffer2::from_fn((8, 1), |x, _| {
                        f32::from(u8::from(reaches(frame, c).contains(&x)))
                    });
                    DropPlanes {
                        weight: reached.clone(),
                        confidence: reached,
                    }
                })
                .collect();
            let mad = if frame == 2 { 1.0 } else { 2.0 };
            let mut admitted = channel_stats(&[(0.0, mad); 3]);
            let stats = &mut admitted.stats;
            stats.spread = Some(
                stats
                    .bg
                    .iter()
                    .map(|&bg| SlotSpread {
                        sigma: bg.sigma(),
                        samples: 4096,
                    })
                    .collect(),
            );
            StoredFrame::from_memory(
                LinearImage::from_planar_channels(desc, planes),
                FrameQuality::Drizzled { drops },
                admitted,
            )
        })
        .collect();
    assert_eq!(
        select_reference_frame(&frames, &[DomainMap::IDENTITY; 3]),
        2,
        "the fixture's reference is frame 2"
    );
    let norms = |normalization| {
        FrameNorm::measure(
            SlotPlanes {
                frames: &frames,
                desc,
                slots: Slots::of_frames(&frames, desc),
                cancel: &CancelToken::never(),
            },
            normalization,
        )
        .unwrap()
        .unwrap()
    };

    let global = norms(Normalization::Global);
    let multiplicative = norms(Normalization::Multiplicative);
    for (frame, &(a, b)) in affine.iter().enumerate() {
        for c in 0..3 {
            let shared = reaches(frame, c);
            let middle = reference(c, shared.start.max(1) + 1);
            assert_eq!(
                global[frame].slots[c],
                SlotNorm {
                    gain: 1.0 / a,
                    offset: -b / a,
                },
                "frame {frame} channel {c}: global"
            );
            assert_eq!(
                multiplicative[frame].slots[c],
                SlotNorm {
                    gain: middle / (a * middle + b),
                    offset: 0.0,
                },
                "frame {frame} channel {c}: multiplicative"
            );
        }
    }
    assert!(
        global[2]
            .slots
            .iter()
            .all(|&slot| slot == SlotNorm::IDENTITY)
    );
}

/// A pair too thin to fit gives way to the per-frame norms for the whole set; one that fits exactly
/// keeps the pairs. Three 6×1 RGB frames: the reference (frame 0) reaches every pixel at
/// `r = x + 1`, frame 1 pixels 0 to 2 at `2r + 1`, frame 2 pixels 3 to 5 at `r/2 − 1/4`, so each
/// pair shares three pixels. Their source statistics are the 3-4-5 figures of the test below, over
/// 4096 samples each, whose per-frame gains state errors of about 4%: frame 1's scale
/// `√(5² − 3²) = 4` carries `√(5.442·(5⁴ + 3⁴)/4096)/(2·16) = 3.0%` and the reference's 3.0%.
///
/// - Jittered by 0.3, −0.5 and 0.4 off its line, frame 1's pair fits over three pixels to an error
///   past that, and every frame takes its per-frame norm: Global (0.125, 1) and (0.5, 0),
///   Multiplicative 1/4 and 1/2, as below.
/// - On their exact lines both pairs fit with no error, and the pairs stand: Global (1/2, −1/2) and
///   (2, 1/2).
#[test]
fn a_pair_too_thin_to_fit_yields_to_the_per_frame_norms() {
    let desc = PlanarDesc::new((6, 1), ChannelLayout::Rgb);
    let frames = |jitter: [f32; 3]| -> Vec<StoredFrame> {
        let shape = [(1.0f32, 0.0f32, 0..6), (2.0, 1.0, 0..3), (0.5, -0.25, 3..6)];
        let figures = [
            (2.0f32, 0.375f32, 0.625f32),
            (8.0, 3.0, 5.0),
            (4.0, 0.75, 1.25),
        ];
        shape
            .into_iter()
            .zip(figures)
            .enumerate()
            .map(|(idx, ((a, b, reach), (median, noise, spread)))| {
                let plane: Vec<f32> = (0..6)
                    .map(|x| {
                        if !reach.contains(&x) {
                            return 0.0;
                        }
                        let shift = if idx == 1 { jitter[x] } else { 0.0 };
                        a * (x + 1) as f32 + b + shift
                    })
                    .collect();
                let reached: Vec<f32> = (0..6)
                    .map(|x| f32::from(u8::from(reach.contains(&x))))
                    .collect();
                let drops = (0..3)
                    .map(|_| DropPlanes {
                        weight: Buffer2::new((6, 1), reached.clone()),
                        confidence: Buffer2::new((6, 1), reached.clone()),
                    })
                    .collect();
                let mut admitted = channel_stats(&[(median, 0.0); 3]);
                let stats = &mut admitted.stats;
                stats.bg = [BgSplit::of_sigma(noise); 3].into_iter().collect();
                stats.spread = Some(
                    [SlotSpread {
                        sigma: spread,
                        samples: 4096,
                    }; 3]
                        .into_iter()
                        .collect(),
                );
                StoredFrame::from_memory(
                    LinearImage::from_planar_channels(desc, vec![plane; 3]),
                    FrameQuality::Drizzled { drops },
                    admitted,
                )
            })
            .collect()
    };
    let norms = |frames: &[StoredFrame], normalization| {
        FrameNorm::measure(
            SlotPlanes {
                frames,
                desc,
                slots: Slots::of_frames(frames, desc),
                cancel: &CancelToken::never(),
            },
            normalization,
        )
        .unwrap()
        .unwrap()
    };
    let expect = |measured: Vec<FrameNorm>, expected: [(f32, f32); 3], case: &str| {
        for (idx, (norm, (gain, offset))) in measured.iter().zip(expected).enumerate() {
            assert!(
                norm.slots
                    .iter()
                    .all(|&slot| slot == SlotNorm { gain, offset }),
                "{case}: frame {idx}: {:?}",
                norm.slots
            );
        }
    };

    let thin = frames([0.3, -0.5, 0.4]);
    expect(
        norms(&thin, Normalization::Global),
        [(1.0, 0.0), (0.125, 1.0), (0.5, 0.0)],
        "thin, global",
    );
    expect(
        norms(&thin, Normalization::Multiplicative),
        [(1.0, 0.0), (0.25, 0.0), (0.5, 0.0)],
        "thin, multiplicative",
    );
    expect(
        norms(&frames([0.0; 3]), Normalization::Global),
        [(1.0, 0.0), (0.5, -0.5), (2.0, 0.5)],
        "exact, global",
    );
}

/// Frames gathered per channel of which some pair shares no pixel are normalized one by one, from
/// their source statistics: three 4×1 RGB frames, each reaching one pixel of its own in every
/// channel, so no pixel is shared at all. The scale is `√(spread² − σ²)`, chosen on 3-4-5 triangles
/// so every figure is exact: the reference (frame 0, the quietest) has median 2, σ 0.375 and
/// spread 0.625, a scale of 0.5; frame 1 median 8, σ 3, spread 5, a scale of 4; frame 2 median 4,
/// σ 0.75, spread 1.25, a scale of 1.
///
/// Global: gain `0.5/4 = 0.125`, offset `2 − 8·0.125 = 1`; and `0.5`, `2 − 4·0.5 = 0`.
/// Multiplicative: `2/8 = 0.25` and `2/4 = 0.5`. A frame whose spread is its noise holds no signal
/// to scale by and is named; so is one whose excess stays inside three standard errors of the
/// estimate, `3·σ²·√(5.442·2/n)`: at σ 3 over 4096 samples, 1.392, which a spread of 3.2 (an excess
/// of 1.24) does not clear.
#[test]
fn per_channel_frames_sharing_no_pixel_are_normalized_one_by_one() {
    let desc = PlanarDesc::new((4, 1), ChannelLayout::Rgb);
    let frame = |reached: usize, median: f32, noise: f32, spread: f32| {
        let mut admitted = channel_stats(&[(median, 0.0); 3]);
        let stats = &mut admitted.stats;
        stats.bg = [BgSplit::of_sigma(noise); 3].into_iter().collect();
        stats.spread = Some(
            [SlotSpread {
                sigma: spread,
                samples: 4096,
            }; 3]
                .into_iter()
                .collect(),
        );
        let plane: Vec<f32> = (0..4)
            .map(|x| if x == reached { median } else { 0.0 })
            .collect();
        let drops = (0..3)
            .map(|_| {
                let reach = Buffer2::from_fn((4, 1), |x, _| f32::from(u8::from(x == reached)));
                DropPlanes {
                    weight: reach.clone(),
                    confidence: reach,
                }
            })
            .collect();
        StoredFrame::from_memory(
            LinearImage::from_planar_channels(desc, vec![plane; 3]),
            FrameQuality::Drizzled { drops },
            admitted,
        )
    };
    let norms = |frames: &[StoredFrame], normalization| {
        FrameNorm::measure(
            SlotPlanes {
                frames,
                desc,
                slots: Slots::of_frames(frames, desc),
                cancel: &CancelToken::never(),
            },
            normalization,
        )
    };
    let frames = [
        frame(0, 2.0, 0.375, 0.625),
        frame(1, 8.0, 3.0, 5.0),
        frame(2, 4.0, 0.75, 1.25),
    ];
    let expect = |normalization, expected: [(f32, f32); 3]| {
        let measured = norms(&frames, normalization).unwrap().unwrap();
        for (idx, (norm, (gain, offset))) in measured.iter().zip(expected).enumerate() {
            assert!(
                norm.slots
                    .iter()
                    .all(|&slot| slot == SlotNorm { gain, offset }),
                "frame {idx} under {normalization:?}: {:?}",
                norm.slots
            );
        }
    };
    expect(
        Normalization::Global,
        [(1.0, 0.0), (0.125, 1.0), (0.5, 0.0)],
    );
    expect(
        Normalization::Multiplicative,
        [(1.0, 0.0), (0.25, 0.0), (0.5, 0.0)],
    );

    for (spread, case) in [(3.0, "no excess"), (3.2, "an excess inside the error")] {
        let flat = [frame(0, 2.0, 0.375, 0.625), frame(1, 8.0, 3.0, spread)];
        let error = norms(&flat, Normalization::Global).unwrap_err();
        assert!(
            matches!(
                error,
                StackError::NoSignalScale {
                    idx: 1,
                    slot: 0,
                    ..
                }
            ),
            "{case}: {error:?}"
        );
    }
}

/// What measuring the norms holds, counted by hand for three frames of 100 × 100.
///
/// A mono slot samples all 10 000 pixels; the mask is a row of 10 000 bits padded to 79 × 128,
/// 1264 B; a median's histogram is 2¹⁶ counts of 4 bytes, 262 144 B. Over the pixels the frames
/// share, `Global` holds the mask, the slot's indices and the reference's samples at 8 + 4 bytes,
/// 120 000, and on each of two workers a histogram and two copies of 40 000 B of samples:
/// 1264 + 120 000 + 2 × 342 144 = 805 552, and 463 408 on one worker. `Multiplicative` holds the
/// mask and the histograms, 525 552. Pair by pair each worker holds a mask, 80 000 B of indices and
/// a histogram, 343 408, and for `Global` both frames' samples and one more copy, 463 408: twice
/// that on two workers, 686 816 and 926 816. A multiplicative pair reads medians alone, and gathers
/// no samples.
///
/// An RGGB mosaic is three slots of 2500, 5000 and 2500 photosites: indices and reference samples
/// of 120 000 B, the domain's mask and one per color, 5056 B, and on each worker a histogram and
/// two copies of the largest slot's 20 000 B: 5056 + 120 000 + 2 × 302 144 = 729 344.
#[test]
fn the_working_set_is_what_the_measurement_holds() {
    use crate::combine::frame_read::slots::Slots;
    use crate::combine::normalization::Normalization;

    let desc = PlanarDesc::new((100, 100), ChannelLayout::Mono);
    let mono = Slots::new(None, 1);
    let bytes = |slots, normalization, coverage, threads| {
        PlannedNormalization {
            frame_count: 3,
            desc,
            slots,
            normalization,
            coverage,
            threads,
        }
        .heap_bytes()
    };
    for (normalization, coverage, threads, expected) in [
        (Normalization::Global, SetCoverage::Common, 2, 805_552),
        (Normalization::Global, SetCoverage::Common, 1, 463_408),
        (
            Normalization::Multiplicative,
            SetCoverage::Common,
            2,
            525_552,
        ),
        (
            Normalization::Multiplicative,
            SetCoverage::PerChannel,
            2,
            686_816,
        ),
        (Normalization::Global, SetCoverage::PerChannel, 2, 926_816),
        (Normalization::None, SetCoverage::Common, 2, 0),
    ] {
        assert_eq!(
            bytes(mono, normalization, coverage, threads),
            expected,
            "{normalization:?}, {coverage:?}, {threads} workers"
        );
    }
    let rggb = Slots::new(Some(Mosaic::Bayer(BayerPattern::Rggb)), 1);
    assert_eq!(
        bytes(rggb, Normalization::Global, SetCoverage::Common, 2),
        729_344
    );
}
