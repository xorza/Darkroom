use crate::math::statistics::ChannelStats;
use crate::stacking::combine::normalization::*;
use crate::stacking::frame_store::FrameStats;

fn channel_stats(median: f32, mad: f32) -> ChannelStats {
    ChannelStats { median, mad }
}

fn frame_stats(median: f32, mad: f32) -> FrameStats {
    FrameStats {
        channels: [channel_stats(median, mad)].into_iter().collect(),
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
        },
        FrameStats {
            channels: [
                channel_stats(100.0, 2.0),
                channel_stats(100.0, 2.0),
                channel_stats(100.0, 2.0),
            ]
            .into_iter()
            .collect(),
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

    let gain = paired_photometric_gain(&frame, &reference, 1.0, 4.0);
    assert_eq!(gain, 2.0);
}
