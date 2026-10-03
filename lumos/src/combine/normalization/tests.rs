use crate::combine::cache::FrameCache;
use crate::combine::config::{CombineMethod, StackConfig};
use crate::combine::normalization::*;
use crate::combine::rejection::Rejection;
use crate::combine::stack::{StackFrame, stack_images};
use crate::frame_store::frame_facts::FrameFacts;
use crate::frame_store::frame_quality::FrameQuality;
use crate::internals::prelude::*;
use crate::internals::synthetic::patterns;
use crate::internals::synthetic::sky_field::{Sky, SkyField};
use crate::progress::ProgressCallback;

/// Statistics with one `(median, mad)` per channel and nothing else stated.
fn channel_stats(channels: &[(f32, f32)]) -> FrameStats {
    FrameStats {
        channels: channels
            .iter()
            .map(|&(median, mad)| MedianMad { median, mad })
            .collect(),
        quantization_sigma: None,
        facts: FrameFacts {
            domain: None,
            row_order: None,
            cfa_type: None,
        },
    }
}

fn frame_stats(median: f32, mad: f32) -> FrameStats {
    channel_stats(&[(median, mad)])
}

/// Three 5×1 RGB frames covering pixels 1..=3 alone, each channel an exact affine image of the
/// others there, with the source MADs `mads`:
///   ch0: f0 = [2,3,4], f1 = [20,30,40], f2 = [8,9,10]
///   ch1: f0 = [20,30,40], f1 = [2,3,4], f2 = [50,70,90]
///   ch2: f0 = [8,9,10], f1 = [200,300,400], f2 = [30,40,50]
fn affine_rgb_frames(mads: [f32; 3]) -> Vec<StoredFrame> {
    let dimensions = ImageDimensions::new((5, 1), 3);
    let coverage = Buffer2::new(5, 1, vec![0.0, 1.0, 1.0, 1.0, 0.0]);
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
                LinearImage::from_planar_channels(dimensions, channels),
                FrameQuality::from_coverage(coverage.clone()),
                channel_stats(&[(0.0, mad); 3]),
            )
        })
        .collect()
}

#[test]
fn reference_selection_uses_lowest_average_channel_noise() {
    let single_channel = [
        frame_stats(100.0, 2.0),
        frame_stats(100.0, 0.5),
        frame_stats(100.0, 1.0),
    ];
    assert_eq!(select_reference_frame(single_channel.iter()), 1);

    let rgb = [
        channel_stats(&[(100.0, 1.0), (100.0, 1.0), (100.0, 5.0)]),
        channel_stats(&[(100.0, 2.0); 3]),
    ];
    assert_eq!(select_reference_frame(rgb.iter()), 1);

    assert_eq!(select_reference_frame([frame_stats(50.0, 3.0)].iter()), 0);
    let equal = [
        frame_stats(100.0, 1.5),
        frame_stats(200.0, 1.5),
        frame_stats(300.0, 1.5),
    ];
    assert_eq!(select_reference_frame(equal.iter()), 0);
}

/// A noiseless line `2x + 5` with one pair thrown far off it: the window drops that pair and the
/// fit through the rest is the line's slope exactly.
#[test]
fn paired_gain_recovers_scale_after_residual_clipping() {
    let frame: Vec<f32> = (0..101).map(|value| value as f32).collect();
    let mut reference: Vec<f32> = frame.iter().map(|value| value * 2.0 + 5.0).collect();
    reference[50] = 10_000.0;

    let cancel = CancelToken::never();
    let reference_stats = sample_stats(&reference, &cancel).unwrap();
    let gain =
        paired_photometric_gain(&frame, &reference, reference_stats, 1.0, 4.0, &cancel).unwrap();
    assert_eq!(gain, 2.0);
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
    let cancel = CancelToken::never();
    let reference_stats = sample_stats(&reference, &cancel).unwrap();
    let gain = |frame_noise, reference_noise| {
        paired_photometric_gain(
            &frame,
            &reference,
            reference_stats,
            frame_noise,
            reference_noise,
            &cancel,
        )
        .unwrap()
    };
    assert_eq!(gain(1.0, 1.0), 2.369_802_2);
    assert_eq!(gain(1.0, 4.0), 2.347_452_9);
    assert_eq!(gain(4.0, 4.0), gain(1.0, 1.0));
    assert_eq!(gain(0.0, 0.0), gain(1.0, 1.0));
}

/// The norms of whole frames, measured as the cache measures them, as `(gain, offset)` per frame
/// and channel.
fn norms(images: Vec<LinearImage>, normalization: Normalization) -> Vec<(f32, f32)> {
    let cache = FrameCache::from_images(images, Normalization::None);
    FrameNorm::measure(
        &cache.frames,
        cache.core.dimensions,
        normalization,
        &CancelToken::never(),
    )
    .unwrap()
    .unwrap()
    .iter()
    .flat_map(|norm| {
        norm.channels
            .iter()
            .map(|channel| (channel.gain, channel.offset))
    })
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
///   Their MADs are 25 steps, 12.5, 1.5625 and 3.125, so frame 1 is the reference, and the seeds
///   1.5625/12.5 = 1/8 and 1.5625/3.125 = 1/2 leave every residual 0. The offsets put the medians
///   104.75, 201.09375 and 146.1875 together: 201.09375 − 104.75/8 = 188, and − 146.1875/2 = 128.
/// - Multiplicative never shifts, whatever the frames.
#[test]
fn frames_related_exactly_normalize_exactly() {
    let uniform = |values: &[f32]| {
        values
            .iter()
            .map(|&value| {
                LinearImage::from_pixels(ImageDimensions::new((16, 1), 1), vec![value; 16])
            })
            .collect::<Vec<_>>()
    };
    let ramp = |count: usize, start: f32, step: f32| {
        LinearImage::from_pixels(
            ImageDimensions::new((count, 1), 1),
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
        [(0.125, 188.0), (1.0, 0.0), (0.5, 128.0)]
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
                    ImageDimensions::new((4, 1), 3),
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
        let product = stack_images(
            frames.into_iter().map(StackFrame::from).collect(),
            &StackConfig {
                method: CombineMethod::Mean(Rejection::None),
                normalization,
                ..Default::default()
            },
            ProgressCallback::default(),
            CancelToken::never(),
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
    let dimensions = ImageDimensions::new((4, 1), 1);
    let coverage = Buffer2::new(4, 1, vec![1.0, 0.5, 1e-4, 0.0]);
    let confidence = Buffer2::new(4, 1, vec![1.0, 2.0, 4.0, 0.0]);
    let image = LinearImage::from_pixels(dimensions, vec![0.5; 4]);
    let frames = vec![StoredFrame::from_memory(
        image,
        FrameQuality::Planes {
            coverage,
            confidence,
        },
        frame_stats(0.5, 0.1),
    )];

    let domain = CommonDomain::build(&frames, dimensions.pixel_count(), &CancelToken::never())
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
    let dimensions = ImageDimensions::new((5, 1), 3);
    let frames = affine_rgb_frames([3.0, 2.0, 1.0]);
    assert_eq!(
        select_reference_frame(frames.iter().map(|frame| &frame.source_stats)),
        2,
        "the fixture must not select frame 0, or it proves nothing"
    );

    let norms = FrameNorm::measure(
        &frames,
        dimensions,
        Normalization::Global,
        &CancelToken::never(),
    )
    .unwrap()
    .expect("global normalization returns parameters");

    let expected = [
        [(1.0, 6.0), (10.0, 0.0), (5.0, 5.0)],
        [(0.1, 6.0), (100.0, 0.0), (0.5, 5.0)],
        [(1.0, 0.0), (1.0, 0.0), (1.0, 0.0)],
    ];
    for (frame_index, frame) in norms.iter().enumerate() {
        for (channel, &(gain, offset)) in expected[frame_index].iter().enumerate() {
            assert_eq!(
                frame.channels[channel].gain, gain,
                "frame {frame_index} channel {channel} gain"
            );
            assert_eq!(
                frame.channels[channel].offset, offset,
                "frame {frame_index} channel {channel} offset"
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
    let dimensions = ImageDimensions::new((5, 1), 3);
    let frames = affine_rgb_frames([1.0; 3]);
    let norms = |normalization| {
        FrameNorm::measure(&frames, dimensions, normalization, &CancelToken::never())
            .unwrap()
            .unwrap()
    };

    let expected_gains = [
        [1.0, 1.0, 1.0],
        [3.0 / 30.0, 30.0 / 3.0, 7.0 / 70.0],
        [3.0f32 / 9.0, 30.0 / 300.0, 7.0 / 40.0],
    ];
    for (frame_index, frame) in norms(Normalization::Multiplicative).iter().enumerate() {
        for (channel, &gain) in expected_gains[frame_index].iter().enumerate() {
            assert_eq!(
                frame.channels[channel],
                ChannelNorm { gain, offset: 0.0 },
                "frame {frame_index} channel {channel}"
            );
        }
    }

    let expected_norms = [
        [(1.0, 0.0), (1.0, 0.0), (1.0, 0.0)],
        [(0.1, 0.0), (10.0, 0.0), (0.1, 0.0)],
        [(1.0, -6.0), (0.1, 0.0), (0.2, -1.0)],
    ];
    for (frame_index, frame) in norms(Normalization::Global).iter().enumerate() {
        for (channel, &(gain, offset)) in expected_norms[frame_index].iter().enumerate() {
            assert_eq!(
                frame.channels[channel],
                ChannelNorm { gain, offset },
                "frame {frame_index} channel {channel}"
            );
        }
    }

    let cancel = CancelToken::new();
    cancel.cancel();
    let error =
        FrameNorm::measure(&frames, dimensions, Normalization::Global, &cancel).unwrap_err();
    assert!(matches!(error, Error::Cancelled));
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
        noise: 0.0,
        clamp: false,
    };
    let truth = SkyField::render(size, sky, 1.5, &stars, 0).pixels;
    let dimensions = ImageDimensions::new((size.width, size.height), 1);
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
            LinearImage::from_pixels(dimensions, pixels)
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
                        FrameQuality::from_coverage(Buffer2::new(size.width, size.height, coverage))
                    }
                    _ => FrameQuality::None,
                };
                StoredFrame::from_memory(image.clone(), quality, FrameStats::measure(image))
            })
            .collect()
    };
    let gains = |frames: &[StoredFrame]| -> Vec<f32> {
        FrameNorm::measure(
            frames,
            dimensions,
            Normalization::Global,
            &CancelToken::never(),
        )
        .unwrap()
        .unwrap()
        .iter()
        .map(|norm| norm.channels[0].gain)
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

/// Past `PHOTOMETRIC_SAMPLE_LIMIT` measured pixels the samples are those of rank `⌊k·n/m⌋`. Over
/// 200 000 pixels with no domain the k-th of 65 536 is `⌊k·3.0518⌋`: 0, 3, 6, 9, …, and 199 996
/// last. Over a domain of the 100 000 even pixels the rank is `⌊k·1.5259⌋` — 0, 1, 3, 4 — and the
/// pixel twice that: 0, 2, 6, 8, …, 199 996 last. Ten pixels are under the limit: every one.
#[test]
fn samples_spread_evenly_by_rank_past_the_limit() {
    let cancel = CancelToken::never();
    let pixel_count = 200_000;
    let check = |indices: Vec<usize>, first: [usize; 4], last: usize| {
        assert_eq!(indices.len(), PHOTOMETRIC_SAMPLE_LIMIT);
        assert_eq!(indices[..4], first);
        assert_eq!(indices[indices.len() - 1], last);
        assert!(indices.is_sorted() && indices.windows(2).all(|pair| pair[0] < pair[1]));
    };
    check(
        stratified_indices(pixel_count, None, &cancel).unwrap(),
        [0, 3, 6, 9],
        199_996,
    );

    let even = (0..pixel_count)
        .map(|pixel| if pixel % 2 == 0 { 1.0 } else { 0.0 })
        .collect();
    let frame = StoredFrame::from_memory(
        LinearImage::from_pixels(
            ImageDimensions::new((pixel_count, 1), 1),
            vec![0.5; pixel_count],
        ),
        FrameQuality::from_coverage(Buffer2::new(pixel_count, 1, even)),
        frame_stats(0.5, 0.1),
    );
    let domain = CommonDomain::build(&[frame], pixel_count, &cancel).unwrap();
    assert_eq!(domain.sample_count, 100_000);
    check(
        stratified_indices(pixel_count, Some(&domain), &cancel).unwrap(),
        [0, 2, 6, 8],
        199_996,
    );

    assert_eq!(
        stratified_indices(10, None, &cancel).unwrap(),
        (0..10).collect::<Vec<_>>()
    );
}

/// A warped frame's noise variance at the samples is its source σ² scaled by the mean inverse
/// confidence there: interpolation that averaged pixels left less noise in each. Confidences 1,
/// 1/2, 1/4 and 1 average an inverse of (1 + 2 + 4 + 1)/4 = 2, every step a power of two, so the
/// variance is exactly 2σ². A frame with no confidence plane keeps σ².
#[test]
fn noise_variance_scales_by_the_mean_inverse_confidence() {
    let cancel = CancelToken::never();
    let image = LinearImage::from_pixels(ImageDimensions::new((4, 1), 1), vec![0.5; 4]);
    let stats = frame_stats(0.5, 0.25);
    let sigma = f64::from(mad_to_sigma(0.25));
    let warped = StoredFrame::from_memory(
        image.clone(),
        FrameQuality::Planes {
            coverage: Buffer2::new(4, 1, vec![1.0; 4]),
            confidence: Buffer2::new(4, 1, vec![1.0, 0.5, 0.25, 1.0]),
        },
        stats.clone(),
    );
    let unwarped = StoredFrame::from_memory(image, FrameQuality::None, stats);
    let indices = [0, 1, 2, 3];
    assert_eq!(
        source_noise_variance(&warped, 0, &indices, 4, &cancel).unwrap(),
        2.0 * sigma * sigma
    );
    assert_eq!(
        source_noise_variance(&unwarped, 0, &indices, 4, &cancel).unwrap(),
        sigma * sigma
    );
}
