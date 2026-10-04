use crate::internals::assertions::assert_close;
use crate::io::raw::demosaic::xtrans::internals::{test_pattern, test_pattern_array};
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPatternError;
use crate::io::raw::demosaic::xtrans::*;
use crate::math::vec2us::Vec2us;

#[test]
fn xtrans_pattern_color_at() {
    let pattern = test_pattern();
    // Check corners
    assert_eq!(pattern.color_at(Vec2us::new(0, 0)), 1); // G
    assert_eq!(pattern.color_at(Vec2us::new(2, 0)), 0); // R
    assert_eq!(pattern.color_at(Vec2us::new(5, 0)), 2); // B
    assert_eq!(pattern.color_at(Vec2us::new(0, 2)), 2); // B
    // Check wrapping
    assert_eq!(
        pattern.color_at(Vec2us::new(0, 6)),
        pattern.color_at(Vec2us::new(0, 0))
    );
    assert_eq!(
        pattern.color_at(Vec2us::new(6, 0)),
        pattern.color_at(Vec2us::new(0, 0))
    );
    assert_eq!(
        pattern.color_at(Vec2us::new(12, 12)),
        pattern.color_at(Vec2us::new(0, 0))
    );
}

#[test]
fn xtrans_pattern_invalid_metadata() {
    let invalid_value_pattern = [
        [1, 0, 1, 1, 2, 1],
        [2, 1, 3, 0, 1, 0], // 3 is invalid
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ];
    let invalid_value = XTransPattern::new(invalid_value_pattern).unwrap_err();
    assert_eq!(
        invalid_value,
        XTransPatternError::Value {
            row: 1,
            column: 2,
            value: 3,
        }
    );

    let mut pattern = test_pattern_array();
    pattern[0][2] = 1;
    assert_eq!(
        XTransPattern::new(pattern).unwrap_err(),
        XTransPatternError::ColorCounts { actual: [7, 21, 8] }
    );

    let invalid_geometry = XTransPattern::new([
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ])
    .unwrap_err();
    assert_eq!(
        invalid_geometry,
        XTransPatternError::GreenNeighborhood {
            row: 1,
            column: 1,
            neighbors: [1, 0, 3],
        }
    );

    // Deserializing checks the layout too, so a stored pattern cannot come back invalid.
    let stored = common::serialize(&test_pattern(), common::SerdeFormat::Ron).unwrap();
    let restored: XTransPattern = common::deserialize(&stored, common::SerdeFormat::Ron).unwrap();
    assert_eq!(restored, test_pattern());
    let mut corrupted = invalid_value_pattern;
    corrupted[1][2] = 3;
    let stored = common::serialize(&corrupted, common::SerdeFormat::Ron).unwrap();
    assert!(common::deserialize::<XTransPattern>(&stored, common::SerdeFormat::Ron).is_err());
}

/// A frame must hold one sample per pixel of a size that is not empty.
#[test]
fn an_xtrans_image_holds_its_size_in_samples() {
    let image = XTransImage::new(&[0.5; 36], Size2us::new(6, 6), test_pattern());
    assert_eq!(image.size, Size2us::new(6, 6));
    assert_eq!(image.read(1, 2), 0.5);
    for (samples, size) in [(24, Size2us::new(0, 4)), (30, Size2us::new(6, 6))] {
        let data = vec![0.5f32; samples];
        let refused = std::panic::catch_unwind(|| {
            XTransImage::new(&data, size, test_pattern());
        });
        assert!(refused.is_err(), "{samples} samples for {size:?}");
    }
}

/// A uniform frame demosaics to itself, three planes of its size: every candidate averages equal
/// values, which round at most a few times.
#[test]
fn a_uniform_frame_demosaics_to_itself() {
    let rgb = demosaic(
        &[0.5; 144],
        Size2us::new(12, 12),
        test_pattern(),
        MarkesteijnPasses::One,
        &CancelToken::never(),
    )
    .unwrap();
    assert!(rgb.iter().all(|plane| plane.len() == 144));
    for &val in rgb.iter().flatten() {
        assert_close!(val, 0.5, 2e-7, "Expected 0.5, got {val}");
    }
}

#[test]
fn f32_demosaic_preserves_signed_native_samples() {
    let size = Size2us::new(18, 18);
    let pattern = test_pattern();
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|index| (index % 17) as f32 * 0.25 - 2.0)
        .collect();
    let rgb = demosaic(
        &data,
        size,
        pattern,
        MarkesteijnPasses::One,
        &CancelToken::never(),
    )
    .unwrap();
    for y in 0..size.height {
        for x in 0..size.width {
            let channel = pattern.color_at(Vec2us::new(x, y)) as usize;
            let expected = data[y * size.width + x];
            let actual = rgb[channel][y * size.width + x];
            assert_close!(
                actual,
                expected,
                1e-6,
                "native channel {channel} at ({x}, {y}) changed from {expected} to {actual}"
            );
        }
    }
}

/// A uniform pedestal passes through: every candidate is an affine combination of samples, so it
/// shifts by the pedestal, to rounding. And a high-contrast frame keeps its overshoot: the
/// demosaic leaves the range for the caller to clamp.
#[test]
fn f32_demosaic_is_equivariant_to_a_uniform_pedestal() {
    let size = Size2us::new(18, 18);
    let pedestal = 0.375;
    let base: Vec<f32> = (0..size.pixel_count())
        .map(|index| 0.2 + (index * 37 % 101) as f32 / 200.0)
        .collect();
    let shifted: Vec<f32> = base.iter().map(|value| value + pedestal).collect();
    let run = |data: &[f32]| {
        demosaic(
            data,
            size,
            test_pattern(),
            MarkesteijnPasses::One,
            &CancelToken::never(),
        )
        .unwrap()
    };
    let base_rgb = run(&base);
    let shifted_rgb = run(&shifted);
    for (channel, (base_channel, shifted_channel)) in base_rgb.iter().zip(&shifted_rgb).enumerate()
    {
        for (pixel, (&base_value, &shifted_value)) in
            base_channel.iter().zip(shifted_channel).enumerate()
        {
            assert!(
                (shifted_value - base_value - pedestal).abs() < 2e-6,
                "channel {channel} pixel {pixel}: expected pedestal {pedestal}, got {}",
                shifted_value - base_value
            );
        }
    }

    let contrast = Size2us::new(30, 30);
    let blocks: Vec<f32> = (0..contrast.pixel_count())
        .map(|index| {
            let (x, y) = (index % contrast.width, index / contrast.width);
            if (x / 2 + y / 3) % 2 == 0 {
                0.008
            } else {
                0.977
            }
        })
        .collect();
    let rgb = demosaic(
        &blocks,
        contrast,
        test_pattern(),
        MarkesteijnPasses::One,
        &CancelToken::never(),
    )
    .unwrap();
    assert!(
        rgb.iter()
            .flatten()
            .any(|value| !(0.0..=1.0).contains(value)),
        "high-contrast interpolation should retain a legitimate overshoot"
    );
}
