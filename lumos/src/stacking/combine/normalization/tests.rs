use common::CancelToken;
use imaginarium::Buffer2;

use crate::io::image::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::statistics::ChannelStats;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::*;
use crate::stacking::frame_store::{FrameStats, StoredLightFrame};

fn channel_stats(median: f32, mad: f32) -> ChannelStats {
    ChannelStats { median, mad }
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
            StoredLightFrame::from_memory(
                LinearImage::from_planar_channels(dimensions, channels),
                Some(coverage.clone()),
                None,
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

    let RegisteredMeasurements::GlobalNormsToFirst(norms) = measure_registered_frames(
        &frames,
        dimensions,
        Normalization::Global,
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
    let error =
        measure_registered_frames(&frames, dimensions, Normalization::Global, &cancel).unwrap_err();
    assert!(matches!(error, Error::Cancelled));
}
