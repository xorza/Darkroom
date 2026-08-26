use crate::io::raw::demosaic::DemosaicError;
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::io::raw::demosaic::xtrans::XTransNormalization;
use crate::io::raw::demosaic::xtrans::internals::{test_pattern, test_pattern_array};
use crate::io::raw::demosaic::xtrans::*;

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

    let raw_data = vec![0u16; 12 * 12];
    assert_eq!(
        process_xtrans(
            &raw_data,
            SensorLayout {
                raw: Size2us::new(12, 12),
                active: Size2us::new(6, 6),
                margin: Vec2us::new(3, 3),
            },
            invalid_value_pattern,
            XTransNormalization {
                channel_black: [0.0; 3],
                inv_range: 1.0,
                black_repeat: None,
            },
            &CancelToken::never(),
        )
        .unwrap_err(),
        DemosaicError::InvalidXTransPattern(XTransPatternError::Value {
            row: 1,
            column: 2,
            value: 3,
        })
    );

    let calibrated = vec![0.0f32; 12 * 12];
    assert!(matches!(
        process_xtrans_f32(
            &calibrated,
            SensorLayout {
                raw: Size2us::new(12, 12),
                active: Size2us::new(6, 6),
                margin: Vec2us::new(3, 3),
            },
            invalid_value_pattern,
            &CancelToken::never(),
        ),
        Err(DemosaicError::InvalidXTransPattern(
            XTransPatternError::Value {
                row: 1,
                column: 2,
                value: 3,
            }
        ))
    ));
}

#[test]
fn xtrans_image_valid() {
    let data = vec![32768u16; 36];
    let pattern = test_pattern();
    let img = XTransImage::with_margins(
        &data,
        SensorLayout {
            raw: Size2us::new(6, 6),
            active: Size2us::new(4, 4),
            margin: Vec2us::new(1, 1),
        },
        pattern,
        XTransNormalization {
            channel_black: [0.0; 3],
            inv_range: 1.0 / 65535.0,
            black_repeat: None,
        },
    );
    assert_eq!(img.raw, Size2us::new(6, 6));
    assert_eq!(img.active, Size2us::new(4, 4));
    assert_eq!(img.margin, Vec2us::new(1, 1));
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn xtrans_image_zero_width() {
    let data = vec![32768u16; 36];
    let pattern = test_pattern();
    XTransImage::with_margins(
        &data,
        SensorLayout {
            raw: Size2us::new(6, 6),
            active: Size2us::new(0, 4),
            margin: Vec2us::ZERO,
        },
        pattern,
        XTransNormalization {
            channel_black: [0.0; 3],
            inv_range: 1.0 / 65535.0,
            black_repeat: None,
        },
    );
}

#[test]
#[should_panic(expected = "Data length")]
fn xtrans_image_wrong_data_length() {
    let data = vec![32768u16; 30]; // Should be 36
    let pattern = test_pattern();
    let size = Size2us::new(6, 6);
    XTransImage::with_margins(
        &data,
        SensorLayout {
            raw: size,
            active: size,
            margin: Vec2us::ZERO,
        },
        pattern,
        XTransNormalization {
            channel_black: [0.0; 3],
            inv_range: 1.0 / 65535.0,
            black_repeat: None,
        },
    );
}

#[test]
fn process_xtrans_output_size() {
    let raw_data: Vec<u16> = vec![1000; 12 * 12];
    let rgb = process_xtrans(
        &raw_data,
        SensorLayout {
            raw: Size2us::new(12, 12),
            active: Size2us::new(6, 6),
            margin: Vec2us::new(3, 3),
        },
        test_pattern_array(),
        XTransNormalization {
            channel_black: [0.0; 3],
            inv_range: 1.0 / 4096.0,
            black_repeat: None,
        },
        &CancelToken::never(),
    )
    .unwrap();

    assert_eq!(rgb.iter().map(|c| c.len()).sum::<usize>(), 6 * 6 * 3);
}

#[test]
fn process_xtrans_normalization() {
    let black = 256.0;
    let maximum = 4096.0;
    let range = maximum - black;
    let inv_range = 1.0 / range;

    // All values equal black + range/2 = 2176 → normalizes to 0.5
    let mid_value = (black + range / 2.0) as u16;
    let raw_data: Vec<u16> = vec![mid_value; 12 * 12];

    let rgb = process_xtrans(
        &raw_data,
        SensorLayout {
            raw: Size2us::new(12, 12),
            active: Size2us::new(6, 6),
            margin: Vec2us::new(3, 3),
        },
        test_pattern_array(),
        XTransNormalization {
            channel_black: [black; 3],
            inv_range,
            black_repeat: None,
        },
        &CancelToken::never(),
    )
    .unwrap();

    for &val in rgb.iter().flatten() {
        assert!((val - 0.5).abs() < 0.01, "Expected ~0.5, got {}", val);
    }
}

#[test]
fn process_xtrans_clamps_below_black() {
    let black = 256.0;
    let range = 4096.0 - black;
    let inv_range = 1.0 / range;

    // All values below black level
    let raw_data: Vec<u16> = vec![100; 12 * 12];

    let rgb = process_xtrans(
        &raw_data,
        SensorLayout {
            raw: Size2us::new(12, 12),
            active: Size2us::new(6, 6),
            margin: Vec2us::new(3, 3),
        },
        test_pattern_array(),
        XTransNormalization {
            channel_black: [black; 3],
            inv_range,
            black_repeat: None,
        },
        &CancelToken::never(),
    )
    .unwrap();

    for &val in rgb.iter().flatten() {
        assert_eq!(val, 0.0, "Expected 0.0 for values below black level");
    }
}

#[test]
fn process_xtrans_full_range() {
    let black = 0.0;
    let inv_range = 1.0 / 65535.0;

    let raw_data: Vec<u16> = vec![65535; 12 * 12];

    let rgb = process_xtrans(
        &raw_data,
        SensorLayout {
            raw: Size2us::new(12, 12),
            active: Size2us::new(6, 6),
            margin: Vec2us::new(3, 3),
        },
        test_pattern_array(),
        XTransNormalization {
            channel_black: [black; 3],
            inv_range,
            black_repeat: None,
        },
        &CancelToken::never(),
    )
    .unwrap();

    for &val in rgb.iter().flatten() {
        assert!((val - 1.0).abs() < 0.001, "Expected 1.0, got {}", val);
    }
}

#[test]
fn xtrans_normalization_is_per_channel_and_raw_linear() {
    let common_black = 200.0;
    let maximum = 4096.0;
    let inv_range = 1.0 / (maximum - common_black);
    let raw_val = 2000u16;
    let raw_data = vec![raw_val; 6 * 6];
    let size = Size2us::new(6, 6);
    let image = XTransImage::with_margins(
        &raw_data,
        SensorLayout {
            raw: size,
            active: size,
            margin: Vec2us::ZERO,
        },
        test_pattern(),
        XTransNormalization {
            channel_black: [250.0, common_black, 220.0],
            inv_range,
            black_repeat: None,
        },
    );

    let expected_red = (2000.0 - 250.0) / 3896.0;
    let expected_green = (2000.0 - 200.0) / 3896.0;
    let expected_blue = (2000.0 - 220.0) / 3896.0;
    assert!((image.read_normalized(0, 2) - expected_red).abs() < 1e-7);
    assert!((image.read_normalized(0, 0) - expected_green).abs() < 1e-7);
    assert!((image.read_normalized(0, 5) - expected_blue).abs() < 1e-7);
}

#[test]
fn process_xtrans_f32_output_size() {
    let data: Vec<f32> = vec![0.5; 12 * 12];
    let rgb = process_xtrans_f32(
        &data,
        SensorLayout {
            raw: Size2us::new(12, 12),
            active: Size2us::new(6, 6),
            margin: Vec2us::new(3, 3),
        },
        test_pattern_array(),
        &CancelToken::never(),
    )
    .unwrap();
    assert_eq!(rgb.iter().map(|c| c.len()).sum::<usize>(), 6 * 6 * 3);
}

#[test]
fn process_xtrans_f32_uniform() {
    let data: Vec<f32> = vec![0.5; 12 * 12];
    let rgb = process_xtrans_f32(
        &data,
        SensorLayout {
            raw: Size2us::new(12, 12),
            active: Size2us::new(6, 6),
            margin: Vec2us::new(3, 3),
        },
        test_pattern_array(),
        &CancelToken::never(),
    )
    .unwrap();

    for &val in rgb.iter().flatten() {
        assert!((val - 0.5).abs() < 0.01, "Expected ~0.5, got {}", val);
    }
}

#[test]
fn f32_demosaic_preserves_signed_native_samples() {
    let raw_width = 30;
    let raw_height = 30;
    let width = 18;
    let height = 18;
    let top = 6;
    let left = 6;
    let pattern = test_pattern_array();
    let data: Vec<f32> = (0..raw_width * raw_height)
        .map(|index| (index % 17) as f32 * 0.25 - 2.0)
        .collect();

    let rgb = process_xtrans_f32(
        &data,
        SensorLayout {
            raw: Size2us::new(raw_width, raw_height),
            active: Size2us::new(width, height),
            margin: Vec2us::new(left, top),
        },
        pattern,
        &CancelToken::never(),
    )
    .unwrap();

    for y in 0..height {
        for x in 0..width {
            let raw_y = y + top;
            let raw_x = x + left;
            let channel = pattern[raw_y % 6][raw_x % 6] as usize;
            let expected = data[raw_y * raw_width + raw_x];
            let actual = rgb[channel][y * width + x];
            assert!(
                (actual - expected).abs() < 1e-6,
                "native channel {channel} at ({x}, {y}) changed from {expected} to {actual}"
            );
        }
    }
}

#[test]
fn f32_demosaic_is_equivariant_to_a_uniform_pedestal() {
    let raw_width = 30;
    let raw_height = 30;
    let width = 18;
    let height = 18;
    let top = 6;
    let left = 6;
    let pedestal = 0.375;
    let base: Vec<f32> = (0..raw_width * raw_height)
        .map(|index| 0.2 + (index * 37 % 101) as f32 / 200.0)
        .collect();
    let shifted: Vec<f32> = base.iter().map(|value| value + pedestal).collect();
    let demosaic = |data: &[f32]| {
        process_xtrans_f32(
            data,
            SensorLayout {
                raw: Size2us::new(raw_width, raw_height),
                active: Size2us::new(width, height),
                margin: Vec2us::new(left, top),
            },
            test_pattern_array(),
            &CancelToken::never(),
        )
        .unwrap()
    };

    let base_rgb = demosaic(&base);
    let shifted_rgb = demosaic(&shifted);
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
}

#[test]
fn process_xtrans_f32_matches_u16_path() {
    let black = 0.0_f32;
    let inv_range = 1.0 / 65535.0_f32;
    let raw_width = 30;
    let raw_height = 30;
    let width = 18;
    let height = 18;
    let margin = 6;
    let raw_u16: Vec<u16> = (0..raw_width * raw_height)
        .map(|index| {
            let y = index / raw_width;
            let x = index % raw_width;
            if (x / 2 + y / 3) % 2 == 0 {
                512
            } else {
                64_000
            }
        })
        .collect();
    let raw_f32: Vec<f32> = raw_u16
        .iter()
        .map(|&v| (v as f32 - black).max(0.0) * inv_range)
        .collect();

    let rgb_u16 = process_xtrans(
        &raw_u16,
        SensorLayout {
            raw: Size2us::new(raw_width, raw_height),
            active: Size2us::new(width, height),
            margin: Vec2us::new(margin, margin),
        },
        test_pattern_array(),
        XTransNormalization {
            channel_black: [black; 3],
            inv_range,
            black_repeat: None,
        },
        &CancelToken::never(),
    )
    .unwrap();
    let rgb_f32 = process_xtrans_f32(
        &raw_f32,
        SensorLayout {
            raw: Size2us::new(raw_width, raw_height),
            active: Size2us::new(width, height),
            margin: Vec2us::new(margin, margin),
        },
        test_pattern_array(),
        &CancelToken::never(),
    )
    .unwrap();

    assert_eq!(
        rgb_u16.iter().flatten().count(),
        rgb_f32.iter().flatten().count()
    );
    for (i, (&a, &b)) in rgb_u16
        .iter()
        .flatten()
        .zip(rgb_f32.iter().flatten())
        .enumerate()
    {
        assert!(
            (a - b).abs() < 1e-5,
            "Pixel {i}: u16 path={a}, f32 path={b}, diff={}",
            (a - b).abs()
        );
    }
    assert!(
        rgb_u16
            .iter()
            .flatten()
            .any(|value| !(0.0..=1.0).contains(value)),
        "high-contrast interpolation should retain a legitimate overshoot"
    );
}
