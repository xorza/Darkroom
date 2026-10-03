use crate::stacking::combine::normalization::*;
use crate::stacking::frame_store::frame_facts::FrameFacts;
use crate::stacking::frame_store::frame_quality::FrameQuality;
use crate::testing::prelude::*;
use crate::testing::synthetic::patterns;
use crate::testing::synthetic::sky_field::{Sky, SkyField};

fn channel_stats(median: f32, mad: f32) -> MedianMad {
    MedianMad { median, mad }
}

fn frame_stats(median: f32, mad: f32) -> FrameStats {
    FrameStats {
        channels: [channel_stats(median, mad)].into_iter().collect(),
        quantization_sigma: None,
        facts: FrameFacts {
            domain: None,
            row_order: None,
            cfa_type: None,
        },
    }
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
        FrameStats {
            channels: [
                channel_stats(100.0, 1.0),
                channel_stats(100.0, 1.0),
                channel_stats(100.0, 5.0),
            ]
            .into_iter()
            .collect(),
            quantization_sigma: None,
            facts: FrameFacts {
                domain: None,
                row_order: None,
                cfa_type: None,
            },
        },
        FrameStats {
            channels: [
                channel_stats(100.0, 2.0),
                channel_stats(100.0, 2.0),
                channel_stats(100.0, 2.0),
            ]
            .into_iter()
            .collect(),
            quantization_sigma: None,
            facts: FrameFacts {
                domain: None,
                row_order: None,
                cfa_type: None,
            },
        },
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
    let dimensions = ImageDimensions::new((5, 1), 3);
    let coverage = Buffer2::new(5, 1, vec![0.0, 1.0, 1.0, 1.0, 0.0]);
    // Common domain is pixels 1..=3. Per channel the three frames are affine images of frame 2:
    //   ch0: f0 = [2,3,4], f1 = [20,30,40], f2 = [8,9,10]   → f0·1 + 6, f1·0.1 + 6
    //   ch1: f0 = [20,30,40], f1 = [2,3,4], f2 = [200,300,400] → f0·10 + 0, f1·100 + 0
    //   ch2: f0 = [5,7,9], f1 = [50,70,90], f2 = [30,40,50] → f0·5 + 5, f1·0.5 + 5
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
    // Frame 2 is the least noisy, so `select_reference_frame` picks it.
    let source_mads = [3.0f32, 2.0, 1.0];
    let frames = channels
        .into_iter()
        .zip(source_mads)
        .map(|(channels, mad)| {
            StoredFrame::from_memory(
                LinearImage::from_planar_channels(dimensions, channels),
                FrameQuality::from_coverage(coverage.clone()),
                FrameStats {
                    channels: [channel_stats(0.0, mad); 3].into_iter().collect(),
                    quantization_sigma: None,
                    facts: FrameFacts {
                        domain: None,
                        row_order: None,
                        cfa_type: None,
                    },
                },
            )
        })
        .collect::<Vec<_>>();
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
    let frames = channels
        .into_iter()
        .map(|channels| {
            StoredFrame::from_memory(
                LinearImage::from_planar_channels(dimensions, channels),
                FrameQuality::from_coverage(coverage.clone()),
                FrameStats {
                    channels: [channel_stats(0.0, 1.0); 3].into_iter().collect(),
                    quantization_sigma: None,
                    facts: FrameFacts {
                        domain: None,
                        row_order: None,
                        cfa_type: None,
                    },
                },
            )
        })
        .collect::<Vec<_>>();
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
/// ratio of sky spreads, the old seed and the old estimator for unregistered frames, misses
/// every gain by far. The fit must find each within its own noise: the Deming slope's standard
/// error is `√((σ²_ref + g²σ²_k) / S_xx)`, with `S_xx` the frame's spread over the paired pixels,
/// and 5 of those bound it.
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
