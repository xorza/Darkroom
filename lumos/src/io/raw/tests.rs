use std::fs;

use crate::io::astro_image::cfa::CfaType;
use crate::io::astro_image::error::ImageError;
use crate::testing::ScratchDirectory;

use crate::io::raw::*;

#[test]
fn test_load_raw_invalid_path() {
    let result = load_raw(Path::new("/nonexistent/path/to/file.raf"));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Failed to open file"));
}

#[cfg(unix)]
#[test]
fn test_load_raw_rejects_interior_nul_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(OsStr::from_bytes(b"invalid\0path.raf"));
    let error = load_raw(path).unwrap_err();
    assert!(error.to_string().contains("interior NUL byte"));
}

#[test]
fn test_load_raw_rejects_invalid_files() {
    #[derive(Debug)]
    struct InvalidRawCase {
        name: &'static str,
        contents: &'static [u8],
    }

    let directory = ScratchDirectory::new("invalid_raw_files");
    let cases = [
        InvalidRawCase {
            name: "invalid_data",
            contents: b"not a valid raw file",
        },
        InvalidRawCase {
            name: "empty",
            contents: b"",
        },
    ];

    for case in cases {
        let path = directory.join(format!("{}.raf", case.name));
        fs::write(&path, case.contents).unwrap();
        assert!(load_raw(&path).is_err(), "{case:?}");
    }
}

#[test]
fn malformed_xtrans_metadata_returns_raw_image_error() {
    use crate::io::raw::demosaic::xtrans::test_support::test_pattern_array;

    let path = Path::new("malformed-xtrans.raf");
    let mut pattern = test_pattern_array();
    pattern[1][2] = 3;
    let error = validate_xtrans_pattern(path, pattern).unwrap_err();

    assert!(matches!(
        error,
        ImageError::Raw {
            path: error_path,
            reason,
        } if error_path == path
            && reason
                == "invalid X-Trans pattern value 3 at row 1, column 2; expected 0, 1, or 2"
    ));
}

#[test]
fn direct_raw_boundary_clamps_finished_image_once() {
    let dimensions = ImageDimensions::new((3, 1), 3);
    let mut image = LinearImage::from_planar_channels(
        dimensions,
        [
            vec![-0.25, 0.5, 1.25],
            vec![1.5, -0.5, 0.25],
            vec![0.0, 1.0, 2.0],
        ],
    );

    clamp_direct_raw_image(&mut image);

    assert_eq!(image.channel(0).pixels(), &[0.0, 0.5, 1.0]);
    assert_eq!(image.channel(1).pixels(), &[1.0, 0.0, 0.25]);
    assert_eq!(image.channel(2).pixels(), &[0.0, 1.0, 1.0]);
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn test_load_raw_valid_file() {
    use crate::testing::{first_raw_file, init_tracing};

    let Some(path) = first_raw_file() else {
        eprintln!("No RAW file found for testing, skipping");
        return;
    };

    init_tracing();

    let result = load_raw(&path);
    assert!(result.is_ok(), "Failed to load {:?}: {:?}", path, result);

    let image = result.unwrap();

    // Validate dimensions
    assert!(image.dimensions().width() > 0);
    assert!(image.dimensions().height() > 0);
    assert_eq!(image.dimensions().channels(), 3); // RGB output

    // Validate pixel values are normalized (check all channels)
    for c in 0..3 {
        for &pixel in image.channel(c) {
            assert!(pixel >= 0.0, "Pixel value {} is negative", pixel);
            // Values can exceed 1.0 slightly due to demosaic interpolation overshoot
            assert!(pixel <= 1.5, "Pixel value {} is too large", pixel);
        }
    }

    // Check mean is reasonable (not all zeros or all ones)
    let mean = image.mean();
    assert!(mean > 0.0, "Mean is zero, image may be all black");
    assert!(mean < 1.0, "Mean is >= 1.0, image may be overexposed");
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn test_load_raw_dimensions_match() {
    use crate::testing::first_raw_file;

    let Some(path) = first_raw_file() else {
        eprintln!("No RAW file found for testing, skipping");
        return;
    };

    let image = load_raw(&path).unwrap();

    // Header dimensions should match actual dimensions
    assert_eq!(image.metadata.header_dimensions.len(), 3);
    assert_eq!(
        image.metadata.header_dimensions[0],
        image.dimensions().height()
    );
    assert_eq!(
        image.metadata.header_dimensions[1],
        image.dimensions().width()
    );
    assert_eq!(
        image.metadata.header_dimensions[2],
        image.dimensions().channels()
    );
}

#[test]
fn test_libraw_guard_cleanup() {
    // Test that LibrawGuard properly cleans up
    {
        let inner = unsafe { sys::libraw_init(0) };
        assert!(!inner.is_null());
        let _guard = LibrawGuard(inner);
        // Guard will be dropped here and call libraw_close
    }
    // If we got here without crashing, cleanup worked
}

#[test]
fn test_libraw_guard_null_safe() {
    // Test that LibrawGuard handles null pointer safely
    let _guard = LibrawGuard(std::ptr::null_mut());
    // Should not crash on drop
}

#[test]
fn test_normalize_u16_to_f32_parallel() {
    // Test the SIMD normalization function
    let black = 512.0;
    let maximum = 16383.0;
    let inv_range = 1.0 / (maximum - black);

    // Test data with known values
    let input: Vec<u16> = vec![
        0,     // Below black -> 0.0
        512,   // At black -> 0.0
        8447,  // Midpoint -> ~0.5
        16383, // At maximum -> 1.0
        20000, // Above maximum -> clamped to 1.0
    ];

    let result = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    assert_eq!(result.len(), input.len());

    // Below black should be 0
    assert!((result[0] - 0.0).abs() < 1e-6, "Below black should be 0");
    // At black should be 0
    assert!((result[1] - 0.0).abs() < 1e-6, "At black should be 0");
    // Midpoint should be ~0.5
    assert!(
        (result[2] - 0.5).abs() < 0.01,
        "Midpoint should be ~0.5, got {}",
        result[2]
    );
    // At maximum should be 1.0
    assert!(
        (result[3] - 1.0).abs() < 1e-6,
        "At maximum should be 1.0, got {}",
        result[3]
    );
    // Above maximum should be clamped to 1.0
    assert!(
        (result[4] - 1.0).abs() < 1e-6,
        "Above maximum should be clamped to 1.0, got {}",
        result[4]
    );
}

#[test]
fn test_normalize_u16_large_array() {
    // Test with a large array to exercise parallel processing
    let size = 100_000;
    let input: Vec<u16> = (0..size).map(|i| (i % 65536) as u16).collect();
    let black = 0.0;
    let inv_range = 1.0 / 65535.0;

    let result = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    assert_eq!(result.len(), size);

    // Verify no NaN or infinite values
    for (i, &v) in result.iter().enumerate() {
        assert!(!v.is_nan(), "NaN at index {}", i);
        assert!(v.is_finite(), "Infinite at index {}", i);
        assert!(v >= 0.0, "Negative value at index {}", i);
    }

    // Check first and last values
    assert!((result[0] - 0.0).abs() < 1e-6);
    assert!((result[65535] - 1.0).abs() < 1e-4);
}

#[test]
fn test_normalize_active_area_crops_and_applies_bayer_deltas() {
    let area = RawActiveArea {
        raw_width: 6,
        width: 3,
        height: 2,
        top_margin: 1,
        left_margin: 2,
    };
    let black = 100.0;
    let inv_range = 0.001;
    let filters = 0x94949494;
    let channel_delta = [0.1, 0.2, 0.3, 0.4];
    let mut raw_data = vec![65_535; area.raw_width * 4];
    raw_data[area.raw_width + 2..area.raw_width + 5].copy_from_slice(&[50, 100, 200]);
    raw_data[2 * area.raw_width + 2..2 * area.raw_width + 5].copy_from_slice(&[300, 1100, 1200]);

    let without_delta =
        normalize_active_area::<true>(&raw_data, area, black, inv_range, None, None);
    assert_eq!(without_delta, [0.0, 0.0, 0.1, 0.2, 1.0, 1.0]);

    let clamped = normalize_active_area::<true>(
        &raw_data,
        area,
        black,
        inv_range,
        Some(ChannelBlackDelta::LibRawFilter {
            visible_filters: filters,
            values: channel_delta,
        }),
        None,
    );
    let unclamped = normalize_active_area::<false>(
        &raw_data,
        area,
        black,
        inv_range,
        Some(ChannelBlackDelta::LibRawFilter {
            visible_filters: filters,
            values: channel_delta,
        }),
        None,
    );
    let clamped_expected = [0.0, 0.0, 0.0, 0.0, 0.7, 0.8];
    let unclamped_expected = [-0.15, -0.2, 0.0, 0.0, 0.7, 0.9];
    for (index, ((got_clamped, got_unclamped), (want_clamped, want_unclamped))) in clamped
        .iter()
        .zip(&unclamped)
        .zip(clamped_expected.iter().zip(&unclamped_expected))
        .enumerate()
    {
        assert!(
            (got_clamped - want_clamped).abs() < 1e-6,
            "clamped[{index}] = {got_clamped}, expected {want_clamped}"
        );
        assert!(
            (got_unclamped - want_unclamped).abs() < 1e-6,
            "unclamped[{index}] = {got_unclamped}, expected {want_unclamped}"
        );
    }
}

#[test]
fn direct_and_calibration_normalization_share_raw_linear_color_scale() {
    let raw_width = 3;
    let raw_data = [600; 9];
    let black = 100.0;
    let inv_range = 0.001;
    let filters = 0x94949494;
    let channel_delta = [0.1, 0.02, 0.03, 0.04];
    let visible_pattern = CfaPattern::Rggb;
    let active_cfa = CfaType::Bayer(visible_pattern);

    for top_margin in 0..2 {
        for left_margin in 0..2 {
            let raw_pattern = visible_pattern.at_raw_origin(top_margin, left_margin);
            for raw_y in 0..3 {
                for raw_x in 0..3 {
                    assert_eq!(
                        raw_filter_color(filters, raw_y, raw_x, top_margin, left_margin),
                        raw_pattern.color_at(raw_y, raw_x)
                    );
                }
            }

            let mut direct = normalize::normalize_u16_to_f32_parallel(&raw_data, black, inv_range);
            apply_bayer_black_corrections(
                &mut direct,
                raw_width,
                top_margin,
                left_margin,
                filters,
                &channel_delta,
                None,
            );
            let area = RawActiveArea {
                raw_width,
                width: 2,
                height: 2,
                top_margin,
                left_margin,
            };
            let calibration = normalize_active_area::<false>(
                &raw_data,
                area,
                black,
                inv_range,
                Some(ChannelBlackDelta::LibRawFilter {
                    visible_filters: filters,
                    values: channel_delta,
                }),
                None,
            );

            for y in 0..area.height {
                for x in 0..area.width {
                    let active_channel = active_cfa.color_at(x, y) as usize;
                    assert_eq!(active_channel, libraw_filter_color(filters, y, x));
                    let expected = 0.5 - channel_delta[active_channel];
                    let direct_value = direct[(y + top_margin) * raw_width + x + left_margin];
                    let calibration_value = calibration[y * area.width + x];
                    assert!(
                        (direct_value - expected).abs() < 1e-6,
                        "direct margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                    assert!(
                        (calibration_value - expected).abs() < 1e-6,
                        "calibration margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                }
            }
        }
    }
}

#[test]
fn spatial_black_repeat_uses_visible_coordinates_with_nonzero_margins() {
    let mut cblack = [0u32; 4104];
    cblack[..4].copy_from_slice(&[10, 20, 30, 20]);
    cblack[4] = 2;
    cblack[5] = 3;
    cblack[6..12].copy_from_slice(&[5, 7, 9, 11, 13, 15]);
    let black = consolidate_black_levels(&cblack, 100, 1115, 0x94949494).unwrap();

    assert_eq!(black.common, 115.0);
    assert_eq!(black.per_channel, [115.0, 125.0, 135.0, 125.0]);
    assert!((black.inv_range - 0.001).abs() < 1e-10);
    for (&actual, expected) in black.channel_delta_norm.iter().zip([0.0, 0.01, 0.02, 0.01]) {
        assert!((actual - expected).abs() < 1e-8);
    }
    let repeat = black.repeat.as_ref().unwrap();
    assert_eq!(repeat.width, 3);
    assert_eq!(repeat.height, 2);
    for (&actual, expected) in repeat
        .delta_norm
        .iter()
        .zip([0.0, 0.002, 0.004, 0.006, 0.008, 0.010])
    {
        assert!((actual - expected).abs() < 1e-8);
    }

    let area = RawActiveArea {
        raw_width: 7,
        width: 3,
        height: 2,
        top_margin: 1,
        left_margin: 2,
    };
    let mut raw_data = vec![0u16; area.raw_width * 4];
    raw_data[area.raw_width + 2..area.raw_width + 5].copy_from_slice(&[315, 327, 319]);
    raw_data[2 * area.raw_width + 2..2 * area.raw_width + 5].copy_from_slice(&[331, 343, 335]);

    let mut direct =
        normalize::normalize_u16_to_f32_parallel(&raw_data, black.common, black.inv_range);
    apply_bayer_black_corrections(
        &mut direct,
        area.raw_width,
        area.top_margin,
        area.left_margin,
        0x94949494,
        &black.channel_delta_norm,
        black.repeat.as_ref(),
    );
    let calibration = normalize_active_area::<false>(
        &raw_data,
        area,
        black.common,
        black.inv_range,
        Some(ChannelBlackDelta::LibRawFilter {
            visible_filters: 0x94949494,
            values: black.channel_delta_norm,
        }),
        black.repeat.as_ref(),
    );

    for y in 0..area.height {
        for x in 0..area.width {
            let direct_value =
                direct[(y + area.top_margin) * area.raw_width + x + area.left_margin];
            let calibration_value = calibration[y * area.width + x];
            assert!((direct_value - 0.2).abs() < 1e-7, "direct ({x}, {y})");
            assert!(
                (calibration_value - 0.2).abs() < 1e-7,
                "calibration ({x}, {y})"
            );
        }
    }
}

#[test]
fn xtrans_direct_and_calibration_black_corrections_match() {
    use crate::io::raw::demosaic::xtrans::test_support::test_pattern_array;
    use crate::io::raw::demosaic::xtrans::{XTransImage, XTransPattern};

    let raw_width = 11;
    let raw_height = 11;
    let raw_pattern = test_pattern_array();
    let common_black = 100.0;
    let channel_black = [110.0, 120.0, 130.0];
    let inv_range = 0.001;
    let raw_data = vec![600u16; raw_width * raw_height];
    let repeat = BlackRepeat {
        width: 3,
        height: 2,
        delta_norm: [0.0, 0.002, 0.004, 0.006, 0.008, 0.010].into(),
    };

    for top_margin in 0..6 {
        for left_margin in 0..6 {
            let area = RawActiveArea {
                raw_width,
                width: 6,
                height: 6,
                top_margin,
                left_margin,
            };
            let visible_pattern = std::array::from_fn(|y| {
                std::array::from_fn(|x| raw_pattern[(y + top_margin) % 6][(x + left_margin) % 6])
            });
            let active_cfa = CfaType::XTrans(visible_pattern);
            let direct = XTransImage::with_margins(
                &raw_data,
                raw_width,
                raw_height,
                area.width,
                area.height,
                area.top_margin,
                area.left_margin,
                XTransPattern::new(raw_pattern).unwrap(),
                channel_black,
                inv_range,
                Some(&repeat),
            );
            let calibration = normalize_active_area::<false>(
                &raw_data,
                area,
                common_black,
                inv_range,
                Some(ChannelBlackDelta::XTrans {
                    visible_pattern,
                    values: [0.01, 0.02, 0.03],
                }),
                Some(&repeat),
            );

            for y in 0..area.height {
                for x in 0..area.width {
                    let raw_y = y + area.top_margin;
                    let raw_x = x + area.left_margin;
                    let raw_channel = raw_pattern[raw_y % 6][raw_x % 6] as usize;
                    let visible_channel = visible_pattern[y % 6][x % 6] as usize;
                    let active_channel = active_cfa.color_at(x, y) as usize;
                    assert_eq!(raw_channel, visible_channel);
                    assert_eq!(raw_channel, active_channel);

                    let expected = [0.49, 0.48, 0.47][raw_channel] - repeat.at_visible(y, x);
                    let direct_value = direct.read_normalized(raw_y, raw_x);
                    let calibration_value = calibration[y * area.width + x];
                    assert!(
                        (direct_value - expected).abs() < 1e-7,
                        "direct margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                    assert!(
                        (calibration_value - expected).abs() < 1e-7,
                        "calibration margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn real_xtrans_channel_black_matches_direct_and_calibration_paths() {
    use crate::io::raw::demosaic::xtrans::{XTransImage, XTransPattern};
    use crate::testing::calibration_image_paths;

    let paths = calibration_image_paths("Lights")
        .or_else(|| calibration_image_paths("Flats"))
        .expect("No calibration images found");
    let Some(raw) = paths
        .iter()
        .filter_map(|path| open_raw(path).ok())
        .find(|raw| {
            matches!(raw.sensor_type, SensorType::XTrans)
                && raw
                    .black_level
                    .channel_delta_norm
                    .iter()
                    .take(3)
                    .any(|delta| delta.abs() > f32::EPSILON)
        })
    else {
        eprintln!("No X-Trans test file with nonzero channel black deltas");
        return;
    };
    let raw_data = raw.raw_image_slice().unwrap();
    let pattern = raw.raw_xtrans_pattern();
    let direct = XTransImage::with_margins(
        raw_data,
        raw.raw_width,
        raw.raw_height,
        raw.width,
        raw.height,
        raw.top_margin,
        raw.left_margin,
        XTransPattern::new(pattern).unwrap(),
        [
            raw.black_level.per_channel[0],
            raw.black_level.per_channel[1],
            raw.black_level.per_channel[2],
        ],
        raw.black_level.inv_range,
        raw.black_level.repeat.as_ref(),
    );
    let calibration = raw.extract_cfa_pixels::<false>().unwrap();
    let mut compared = 0usize;

    for y in (0..raw.height).step_by(101) {
        for x in (0..raw.width).step_by(113) {
            let raw_y = y + raw.top_margin;
            let raw_x = x + raw.left_margin;
            let calibration_value = calibration[y * raw.width + x];
            if (0.0..=1.0).contains(&calibration_value) {
                let direct_value = direct.read_normalized(raw_y, raw_x);
                assert!((direct_value - calibration_value).abs() < 1e-7);
                compared += 1;
            }
        }
    }
    assert!(compared > 100);
}

#[test]
fn camera_white_balance_is_canonicalized() {
    let bayer = SensorType::Bayer(CfaPattern::Rggb);
    assert_eq!(
        canonical_camera_white_balance(&bayer, [4.0, 2.0, 3.0, 2.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(&bayer, [2.0, 1.0, 1.5, 0.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(&SensorType::XTrans, [2.0, 1.0, 1.5, 9.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(&SensorType::Monochrome, [2.0, 1.0, 1.5, 1.0]),
        None
    );
}

#[test]
fn invalid_camera_white_balance_is_absent() {
    let invalid = [
        [0.0; 4],
        [2.0, -1.0, 1.5, 1.0],
        [2.0, f32::NAN, 1.5, 1.0],
        [2.0, f32::INFINITY, 1.5, 1.0],
    ];
    let sensor_type = SensorType::Bayer(CfaPattern::Rggb);

    for input in invalid {
        assert!(
            canonical_camera_white_balance(&sensor_type, input).is_none(),
            "{input:?}"
        );
    }
}

/// Test that margin pixels (outside active area) are zero after normalization
/// when raw values are below black level.
#[test]
fn test_normalize_below_black_clamped() {
    let black = 500.0;
    let inv_range = 1.0 / 1000.0;

    // All values below black
    let input: Vec<u16> = vec![0, 100, 200, 499];
    let result = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    for (i, &v) in result.iter().enumerate() {
        assert!(
            v == 0.0,
            "Value below black should be 0.0, got {} at index {}",
            v,
            i
        );
    }
}

/// The calibration path keeps signed, un-clamped values — flooring the
/// sub-pedestal tail at 0 (the light-frame clamp) would bias stacked master
/// dark/bias means upward.
#[test]
fn test_normalize_unclamped_preserves_out_of_range() {
    let black = 500.0;
    let inv_range = 1.0 / 1000.0; // white level = 1500

    // below black, below black, in range, above white
    let input: Vec<u16> = vec![0, 100, 499, 700, 2000];
    let mut unclamped = vec![0.0; input.len()];
    normalize::normalize_u16_to_f32_into::<false>(&input, &mut unclamped, black, inv_range);
    let clamped = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    // Unclamped is the exact affine map (value - black) * inv_range; negatives
    // and >1 are retained.
    let unclamped_expected = [-0.5, -0.4, -0.001, 0.2, 1.5];
    for (i, (&got, &want)) in unclamped.iter().zip(unclamped_expected.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1e-6,
            "unclamped[{i}] = {got}, want {want}"
        );
    }

    // Clamped floors the sub-black tail at exactly 0 and caps the over-white
    // value at exactly 1; the in-range value is identical to the unclamped one.
    let clamped_expected = [0.0, 0.0, 0.0, 0.2, 1.0];
    for (i, (&got, &want)) in clamped.iter().zip(clamped_expected.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1e-6,
            "clamped[{i}] = {got}, want {want}"
        );
    }
}

/// Test process_unknown_libraw_fallback 16-bit normalization formula.
/// We can't call the function directly (needs libraw instance), but we can
/// verify the normalization math it uses: (v as f32) / 65535.0
#[test]
fn test_fallback_16bit_normalization() {
    let test_cases: &[(u16, f32)] = &[
        (0, 0.0),
        (1, 1.0 / 65535.0),
        (32767, 32767.0 / 65535.0),
        (65535, 1.0),
    ];

    for &(input, expected) in test_cases {
        let result = (input as f32) / 65535.0;
        assert!(
            (result - expected).abs() < 1e-6,
            "16-bit norm({}) = {}, expected {}",
            input,
            result,
            expected
        );
    }
}

/// Test process_unknown_libraw_fallback 8-bit normalization formula.
#[test]
fn test_fallback_8bit_normalization() {
    let test_cases: &[(u8, f32)] = &[(0, 0.0), (1, 1.0 / 255.0), (127, 127.0 / 255.0), (255, 1.0)];

    for &(input, expected) in test_cases {
        let result = (input as f32) / 255.0;
        assert!(
            (result - expected).abs() < 1e-6,
            "8-bit norm({}) = {}, expected {}",
            input,
            result,
            expected
        );
    }
}

/// Uniform black: all cblack zero, scalar black only.
#[test]
fn test_consolidate_black_levels_uniform() {
    let cblack = [0u32; 4104];
    // No per-channel, no spatial pattern
    let bl = consolidate_black_levels(&cblack, 512, 16383, 0x94949494).unwrap();

    assert_eq!(bl.common, 512.0);
    assert_eq!(bl.per_channel, [512.0; 4]);
    assert_eq!(bl.channel_delta_norm, [0.0; 4]);
    let expected_inv = 1.0 / (16383.0 - 512.0);
    assert!((bl.inv_range - expected_inv).abs() < 1e-10);
}

/// Per-channel cblack[0..3] nonzero, no spatial pattern.
#[test]
fn test_consolidate_black_levels_per_channel() {
    let mut cblack = [0u32; 4104];
    cblack[0] = 10; // R
    cblack[1] = 5; // G1
    cblack[2] = 15; // B
    cblack[3] = 5; // G2
    // No spatial pattern (cblack[4]==0, cblack[5]==0)

    let bl = consolidate_black_levels(&cblack, 100, 4096, 0x94949494).unwrap();

    // Common minimum across channels is 5, moved to black: 100+5=105
    assert_eq!(bl.common, 105.0);
    // Per-channel: cblack[c]-5 + 105
    assert_eq!(bl.per_channel[0], 110.0); // R: 10-5+105
    assert_eq!(bl.per_channel[1], 105.0); // G1: 5-5+105
    assert_eq!(bl.per_channel[2], 115.0); // B: 15-5+105
    assert_eq!(bl.per_channel[3], 105.0); // G2: 5-5+105

    let inv = 1.0 / (4096.0 - 105.0);
    assert!((bl.inv_range - inv).abs() < 1e-10);
    // delta_norm[c] = (per_channel[c] - common) * inv_range
    assert!((bl.channel_delta_norm[0] - 5.0 * inv).abs() < 1e-6);
    assert!(bl.channel_delta_norm[1].abs() < 1e-10);
    assert!((bl.channel_delta_norm[2] - 10.0 * inv).abs() < 1e-6);
    assert!(bl.channel_delta_norm[3].abs() < 1e-10);
}

/// Bayer 2x2 spatial pattern folded into per-channel values.
#[test]
fn test_consolidate_black_levels_bayer_2x2_fold() {
    let mut cblack = [0u32; 4104];
    // 2x2 spatial pattern
    cblack[4] = 2;
    cblack[5] = 2;
    // Pattern values at spatial positions:
    cblack[6] = 4; // (0,0)
    cblack[7] = 8; // (0,1)
    cblack[8] = 12; // (1,0)
    cblack[9] = 16; // (1,1)

    // RGGB Bayer pattern filter
    // FC mapping for RGGB: (0,0)=R=0, (0,1)=G=1, (1,0)=G->G2=3, (1,1)=B=2
    // Folding: cblack[0]+=4(R), cblack[1]+=8(G1), cblack[3]+=12(G2), cblack[2]+=16(B)
    // After fold: cblack = [4, 8, 16, 12]
    // Common min = 4, subtract: cblack = [0, 4, 12, 8], black = 200+4 = 204
    let filters = 0x94949494u32;
    let bl = consolidate_black_levels(&cblack, 200, 16383, filters).unwrap();

    assert_eq!(bl.common, 204.0);
    assert_eq!(bl.per_channel[0], 204.0); // R: 0 + 204
    assert_eq!(bl.per_channel[1], 208.0); // G1: 4 + 204
    assert_eq!(bl.per_channel[2], 216.0); // B: 12 + 204
    assert_eq!(bl.per_channel[3], 212.0); // G2: 8 + 204

    let inv = 1.0 / (16383.0 - 204.0);
    assert!((bl.inv_range - inv).abs() < 1e-10);
    assert!(bl.channel_delta_norm[0].abs() < 1e-10); // R: no delta
    assert!((bl.channel_delta_norm[1] - 4.0 * inv).abs() < 1e-6); // G1
    assert!((bl.channel_delta_norm[2] - 12.0 * inv).abs() < 1e-6); // B
    assert!((bl.channel_delta_norm[3] - 8.0 * inv).abs() < 1e-6); // G2
}

/// X-Trans 1x1 spatial pattern folded into all channels.
#[test]
fn test_consolidate_black_levels_xtrans_1x1_fold() {
    let mut cblack = [0u32; 4104];
    cblack[4] = 1;
    cblack[5] = 1;
    cblack[6] = 20; // Added to all channels

    // X-Trans filter value (typically 9 for 6x6 pattern)
    let bl = consolidate_black_levels(&cblack, 256, 4096, 9).unwrap();

    // 1x1 pattern: cblack[6]=20 added to all cblack[0..3]
    // Then common minimum extracted (all equal = 20), moved to black: 256+20=276
    assert_eq!(bl.common, 276.0);
    assert_eq!(bl.per_channel, [276.0; 4]);
    assert_eq!(bl.channel_delta_norm, [0.0; 4]);
}

#[test]
fn consolidate_black_levels_rejects_invalid_metadata() {
    let cblack = [0u32; 4104];
    let error = consolidate_black_levels(&cblack, 512, 512, 0x94949494).unwrap_err();
    assert!(error.contains("common black 512 >= maximum 512"));

    let mut oversized = [0u32; 4104];
    oversized[4] = 64;
    oversized[5] = 65;
    let error = consolidate_black_levels(&oversized, 0, 4096, 0x94949494).unwrap_err();
    assert!(error.contains("spatial black pattern 65x64 exceeds 4098 entries"));
}

#[test]
fn test_libraw_filter_color_rggb() {
    // RGGB Bayer pattern: 0x94949494
    let filters = 0x94949494u32;

    // (0,0)=R=0, (0,1)=G=1, (1,0)=G=1, (1,1)=B=2
    assert_eq!(libraw_filter_color(filters, 0, 0), 0); // R
    assert_eq!(libraw_filter_color(filters, 0, 1), 1); // G
    assert_eq!(libraw_filter_color(filters, 1, 0), 1); // G
    assert_eq!(libraw_filter_color(filters, 1, 1), 2); // B

    // Pattern repeats
    assert_eq!(libraw_filter_color(filters, 2, 0), 0);
    assert_eq!(libraw_filter_color(filters, 2, 1), 1);
    assert_eq!(libraw_filter_color(filters, 3, 0), 1);
    assert_eq!(libraw_filter_color(filters, 3, 1), 2);
}

#[test]
fn test_apply_bayer_black_corrections_identity() {
    let mut data = vec![0.5f32; 4];
    let delta = [0.0; 4];

    apply_bayer_black_corrections(&mut data, 2, 0, 0, 0x94949494, &delta, None);

    // No change expected
    for &v in &data {
        assert!((v - 0.5).abs() < 1e-6);
    }
}

#[test]
fn test_apply_bayer_black_corrections() {
    // 2x2 RGGB: positions (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    let mut data = vec![0.5f32; 4];
    let delta = [0.1, 0.0, 0.05, 0.0]; // R has delta=0.1, B has delta=0.05

    apply_bayer_black_corrections(&mut data, 2, 0, 0, 0x94949494, &delta, None);

    assert!(
        (data[0] - 0.4).abs() < 1e-6,
        "R: 0.5-0.1=0.4, got {}",
        data[0]
    );
    assert!((data[1] - 0.5).abs() < 1e-6, "G: no delta, got {}", data[1]);
    assert!((data[2] - 0.5).abs() < 1e-6, "G: no delta, got {}", data[2]);
    assert!(
        (data[3] - 0.45).abs() < 1e-6,
        "B: 0.5-0.05=0.45, got {}",
        data[3]
    );
}

#[test]
fn test_apply_bayer_black_corrections_clamp_negative() {
    let mut data = vec![0.05f32; 4];
    let delta = [0.1, 0.0, 0.0, 0.0]; // R delta bigger than value

    apply_bayer_black_corrections(&mut data, 2, 0, 0, 0x94949494, &delta, None);

    // R at (0,0): (0.05 - 0.1).max(0.0) = 0.0
    assert_eq!(data[0], 0.0, "Should clamp to 0.0");
}
