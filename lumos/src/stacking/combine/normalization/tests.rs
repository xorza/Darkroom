use crate::math::statistics::MedianMad;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::photometric_gain::{
    paired_photometric_gain, sample_stats,
};
use crate::stacking::combine::normalization::*;
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::warp_quality::WarpQuality;
use crate::testing::prelude::*;

fn channel_stats(median: f32, mad: f32) -> MedianMad {
    MedianMad { median, mad }
}

fn frame_stats(median: f32, mad: f32) -> FrameStats {
    FrameStats {
        channels: [channel_stats(median, mad)].into_iter().collect(),
        quantization_sigma: None,
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
        WarpQuality::Planes {
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
                WarpQuality::from_coverage(coverage.clone()),
                FrameStats {
                    channels: [channel_stats(0.0, mad); 3].into_iter().collect(),
                    quantization_sigma: None,
                },
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        select_reference_frame(frames.iter().map(|frame| &frame.source_stats)),
        2,
        "the fixture must not select frame 0, or it proves nothing"
    );

    let norms = compute_frame_norms(
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

#[test]
fn registered_rgb_measurements_preserve_pair_order_and_honor_cancellation() {
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
        .enumerate()
        .map(|(frame_index, channels)| {
            StoredFrame::from_memory(
                LinearImage::from_planar_channels(dimensions, channels),
                WarpQuality::from_coverage(coverage.clone()),
                FrameStats {
                    channels: [channel_stats(0.0, 1.0); 3].into_iter().collect(),
                    quantization_sigma: Some((frame_index + 1) as f32),
                },
            )
        })
        .collect::<Vec<_>>();

    let RegisteredMeasurements::CommonStats(measured) = measure_registered_frames(
        &frames,
        dimensions,
        Normalization::Multiplicative,
        0,
        &CancelToken::never(),
    )
    .unwrap() else {
        panic!("multiplicative normalization must return common-domain statistics");
    };
    let expected = [
        [(3.0, 1.0), (30.0, 10.0), (7.0, 2.0)],
        [(30.0, 10.0), (3.0, 1.0), (70.0, 20.0)],
        [(9.0, 1.0), (300.0, 100.0), (40.0, 10.0)],
    ];
    for (frame_index, frame) in measured.iter().enumerate() {
        assert_eq!(frame.quantization_sigma, Some((frame_index + 1) as f32));
        for (channel, &(median, mad)) in expected[frame_index].iter().enumerate() {
            assert_eq!(
                frame.channels[channel].median, median,
                "frame {frame_index} channel {channel} median"
            );
            assert_eq!(
                frame.channels[channel].mad, mad,
                "frame {frame_index} channel {channel} MAD"
            );
        }
    }

    let RegisteredMeasurements::GlobalNorms(norms) = measure_registered_frames(
        &frames,
        dimensions,
        Normalization::Global,
        0,
        &CancelToken::never(),
    )
    .unwrap() else {
        panic!("global normalization must return affine parameters");
    };
    let expected_norms = [
        [(1.0, 0.0), (1.0, 0.0), (1.0, 0.0)],
        [(0.1, 0.0), (10.0, 0.0), (0.1, 0.0)],
        [(1.0, -6.0), (0.1, 0.0), (0.2, -1.0)],
    ];
    for (frame_index, frame) in norms.iter().enumerate() {
        for (channel, &(gain, offset)) in expected_norms[frame_index].iter().enumerate() {
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

    let cancel = CancelToken::new();
    cancel.cancel();
    let error = measure_registered_frames(&frames, dimensions, Normalization::Global, 0, &cancel)
        .unwrap_err();
    assert!(matches!(error, Error::Cancelled));
}
