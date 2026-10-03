use common::TempDir;

use crate::internals::assertions::{assert_close, assert_close_slice};
use crate::internals::cfa::XTRANS_PATTERN;

use crate::io::raw::*;
use std::array;

#[test]
fn load_raw_invalid_path() {
    let path = Path::new("/nonexistent/path/to/file.raf");
    // A path that cannot be read is an `Io` error; a readable file libraw
    // refuses is a `Raw` one, pinned by `load_raw_rejects_invalid_files`.
    let error = load_raw(path, &LoadContext::default()).unwrap_err();
    assert!(
        matches!(&error, ImageError::Io { path: io_path, .. } if io_path == path),
        "a missing file should read as an Io error, got: {error}",
    );

    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = LoadContext::new(cancel, u64::MAX);
    assert!(matches!(
        load_raw(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
    assert!(matches!(
        load_raw_cfa(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
    assert!(matches!(
        raw_cfa_frame_info(path, &cancelled),
        Err(ImageError::Cancelled { path: error_path }) if error_path == path
    ));
}

#[cfg(unix)]
#[test]
fn load_raw_rejects_interior_nul_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(OsStr::from_bytes(b"invalid\0path.raf"));
    let error = load_raw(path, &LoadContext::default()).unwrap_err();
    assert!(error.to_string().contains("interior NUL byte"));
}

#[test]
fn load_raw_rejects_invalid_files() {
    #[derive(Debug)]
    struct InvalidRawCase {
        name: &'static str,
        contents: &'static [u8],
    }

    let directory = TempDir::new("invalid_raw_files");
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
        assert!(
            load_raw(&path, &LoadContext::default()).is_err(),
            "{case:?}"
        );

        // The rejection happens while libraw is being opened, so the state exists and has to free
        // itself on the way out — the failure path most likely to leak the instance.
        //
        // The file reads back, so what libraw refuses is its contents: a `Raw` error on every
        // platform, worded alike whether libraw was handed the path or the bytes.
        let error = LibrawState::open(&path).unwrap_err();
        assert!(
            matches!(&error, ImageError::Raw { .. }),
            "{case:?}: contents libraw refuses should read as a Raw error, got: {error}",
        );
        assert!(
            error.to_string().contains("Failed to open file"),
            "{case:?}: {error}",
        );
    }
}

#[test]
fn interpolated_planes_clamp_to_the_light_frame_range() {
    let mut planes = [
        vec![-0.25, 0.5, 1.25],
        vec![1.5, -0.5, 0.25],
        vec![0.0, 1.0, 2.0],
    ];

    clamp_interpolated(&mut planes);

    assert_eq!(planes[0], [0.0, 0.5, 1.0]);
    assert_eq!(planes[1], [1.0, 0.0, 0.25]);
    assert_eq!(planes[2], [0.0, 1.0, 1.0]);
}

#[test]
fn demosaic_overshoots_the_light_frame_range_it_is_clamped_back_into() {
    use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern, rcd};

    // A saturated square on black: the sharpest edge a CFA can carry, and the case RCD's ratio
    // correction overshoots on. Pins why `clamp_interpolated` exists — if the kernel ever starts
    // bounding its own output, this fails and the clamp becomes dead weight to remove.
    let size = Size2us::new(32, 32);
    let mut cfa = vec![0.0f32; size.pixel_count()];
    for y in 12..20 {
        for x in 12..20 {
            cfa[y * size.width + x] = 1.0;
        }
    }

    let bayer = BayerImage::with_margins(&cfa, SensorLayout::cropped(size), CfaPattern::Rggb);
    let mut planes = rcd::demosaic(&bayer, &CancelToken::never()).unwrap();

    let peak = planes
        .iter()
        .flatten()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(peak > 1.0, "expected an overshoot, got a peak of {peak}");

    clamp_interpolated(&mut planes);
    for (channel, plane) in planes.iter().enumerate() {
        assert!(
            plane.iter().all(|&sample| (0.0..=1.0).contains(&sample)),
            "channel {channel} still leaves [0, 1]"
        );
    }
}

#[cfg(feature = "real-data")]
#[test]
fn load_raw_valid_file() {
    // Check mean is reasonable (not all zeros or all ones)
    use crate::internals::synthetic::metrics::pixel_stats;

    use crate::internals::init_tracing;
    use crate::internals::real_data::raw_frames;

    let path = raw_frames("Lights").swap_remove(0);

    init_tracing();

    let result = load_raw(&path, &LoadContext::default());
    assert!(result.is_ok(), "Failed to load {path:?}: {result:?}");

    let image = result.unwrap();

    // Validate dimensions
    assert!(image.dimensions().width() > 0);
    assert!(image.dimensions().height() > 0);
    assert_eq!(image.dimensions().channels(), 3); // RGB output

    // Every channel is inside the light-frame contract. The demosaic kernels overshoot on their
    // own, so this is what catches a decode path that stopped clamping their output.
    for c in 0..3 {
        for &pixel in image.channel(c) {
            assert!(
                (0.0..=1.0).contains(&pixel),
                "Pixel value {pixel} left [0, 1] in channel {c}"
            );
        }
    }

    let mean = (0..image.channels())
        .map(|channel| pixel_stats(image.channel(channel)).mean)
        .sum::<f64>()
        / image.channels() as f64;
    assert!(mean > 0.0, "Mean is zero, image may be all black");
    assert!(mean < 1.0, "Mean is >= 1.0, image may be overexposed");
}

#[cfg(feature = "real-data")]
#[test]
fn load_raw_dimensions_match() {
    use crate::internals::real_data::raw_frames;

    let path = raw_frames("Lights").swap_remove(0);

    let image = load_raw(&path, &LoadContext::default()).unwrap();

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
fn normalize_active_area_crops_and_applies_bayer_deltas() {
    let layout = SensorLayout {
        raw: Size2us::new(6, 4),
        active: Size2us::new(3, 2),
        margin: Vec2us::new(2, 1),
    };
    let black = 100.0;
    let span = 1000.0;
    let filters = 0x9494_9494;
    let channel_delta = [0.1, 0.2, 0.3, 0.4];
    let mut raw_data = vec![65_535; layout.raw.width * 4];
    raw_data[layout.raw.width + 2..layout.raw.width + 5].copy_from_slice(&[50, 100, 200]);
    raw_data[2 * layout.raw.width + 2..2 * layout.raw.width + 5]
        .copy_from_slice(&[300, 1100, 1200]);

    let without_delta = normalize_active_area::<true>(&raw_data, layout, black, span, None, None);
    assert_eq!(without_delta, [0.0, 0.0, 0.1, 0.2, 1.0, 1.0]);

    let clamped = normalize_active_area::<true>(
        &raw_data,
        layout,
        black,
        span,
        Some(ChannelBlackDelta::LibRawFilter {
            visible_filters: filters,
            values: channel_delta,
        }),
        None,
    );
    let unclamped = normalize_active_area::<false>(
        &raw_data,
        layout,
        black,
        span,
        Some(ChannelBlackDelta::LibRawFilter {
            visible_filters: filters,
            values: channel_delta,
        }),
        None,
    );
    let clamped_expected = [0.0, 0.0, 0.0, 0.0, 0.7, 0.8];
    let unclamped_expected = [-0.15, -0.2, 0.0, 0.0, 0.7, 0.9];
    assert_close_slice!(clamped, clamped_expected, 1e-6, "clamped");
    assert_close_slice!(unclamped, unclamped_expected, 1e-6, "unclamped");
}

#[test]
fn direct_and_calibration_normalization_share_raw_linear_color_scale() {
    let raw_width = 3;
    let raw_data = [600; 9];
    let black = 100.0;
    let span = 1000.0;
    let filters = 0x9494_9494;
    let channel_delta = [0.1, 0.02, 0.03, 0.04];
    let visible_pattern = CfaPattern::Rggb;
    let active_cfa = CfaType::Bayer(visible_pattern);

    for top_margin in 0..2 {
        for left_margin in 0..2 {
            let raw_pattern = visible_pattern.at_raw_origin(top_margin, left_margin);
            for raw_y in 0..3 {
                for raw_x in 0..3 {
                    assert_eq!(
                        raw_filter_color(
                            filters,
                            raw_y,
                            raw_x,
                            Vec2us::new(left_margin, top_margin)
                        ),
                        raw_pattern.color_at(Vec2us::new(raw_x, raw_y))
                    );
                }
            }

            let mut direct = normalize_u16_to_f32_parallel(&raw_data, black, span);
            apply_bayer_black_corrections(
                &mut direct,
                raw_width,
                Vec2us::new(left_margin, top_margin),
                filters,
                &channel_delta,
                None,
            );
            let layout = SensorLayout {
                raw: Size2us::new(raw_width, 3),
                active: Size2us::new(2, 2),
                margin: Vec2us::new(left_margin, top_margin),
            };
            let calibration = normalize_active_area::<false>(
                &raw_data,
                layout,
                black,
                span,
                Some(ChannelBlackDelta::LibRawFilter {
                    visible_filters: filters,
                    values: channel_delta,
                }),
                None,
            );

            for y in 0..layout.active.height {
                for x in 0..layout.active.width {
                    let active_channel = active_cfa.color_at(Vec2us::new(x, y)) as usize;
                    assert_eq!(active_channel, libraw_filter_color(filters, y, x));
                    let expected = 0.5 - channel_delta[active_channel];
                    let direct_value = direct[(y + top_margin) * raw_width + x + left_margin];
                    let calibration_value = calibration[y * layout.active.width + x];
                    assert_close!(
                        direct_value,
                        expected,
                        1e-6,
                        "direct margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                    assert_close!(
                        calibration_value,
                        expected,
                        1e-6,
                        "calibration margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                }
            }
        }
    }
}

#[test]
fn spatial_black_repeat_uses_visible_coordinates_with_nonzero_margins() {
    let mut cblack = no_black();
    cblack[..4].copy_from_slice(&[10, 20, 30, 20]);
    cblack[4] = 2;
    cblack[5] = 3;
    cblack[6..12].copy_from_slice(&[5, 7, 9, 11, 13, 15]);
    let black = consolidate_black_levels(&cblack, 100, 1115, 0x9494_9494).unwrap();

    assert_eq!(black.common, 115.0);
    assert_eq!(black.per_channel, [115.0, 125.0, 135.0, 125.0]);
    assert_eq!(black.span, 1000.0);
    for (&actual, expected) in black.channel_delta_norm.iter().zip([0.0, 0.01, 0.02, 0.01]) {
        assert_close!(actual, expected, 1e-8);
    }
    let repeat = black.repeat.as_ref().unwrap();
    assert_eq!(repeat.size, Size2us::new(3, 2));
    for (&actual, expected) in repeat
        .delta_norm
        .iter()
        .zip([0.0, 0.002, 0.004, 0.006, 0.008, 0.010])
    {
        assert_close!(actual, expected, 1e-8);
    }

    let layout = SensorLayout {
        raw: Size2us::new(7, 4),
        active: Size2us::new(3, 2),
        margin: Vec2us::new(2, 1),
    };
    let mut raw_data = vec![0u16; layout.raw.width * 4];
    raw_data[layout.raw.width + 2..layout.raw.width + 5].copy_from_slice(&[315, 327, 319]);
    raw_data[2 * layout.raw.width + 2..2 * layout.raw.width + 5].copy_from_slice(&[331, 343, 335]);

    let mut direct = normalize_u16_to_f32_parallel(&raw_data, black.common, black.span);
    apply_bayer_black_corrections(
        &mut direct,
        layout.raw.width,
        layout.margin,
        0x9494_9494,
        &black.channel_delta_norm,
        black.repeat.as_ref(),
    );
    let calibration = normalize_active_area::<false>(
        &raw_data,
        layout,
        black.common,
        black.span,
        Some(ChannelBlackDelta::LibRawFilter {
            visible_filters: 0x9494_9494,
            values: black.channel_delta_norm,
        }),
        black.repeat.as_ref(),
    );

    for y in 0..layout.active.height {
        for x in 0..layout.active.width {
            let direct_value =
                direct[(y + layout.margin.y) * layout.raw.width + x + layout.margin.x];
            let calibration_value = calibration[y * layout.active.width + x];
            assert_close!(direct_value, 0.2, 1e-7, "direct ({x}, {y})");
            assert_close!(calibration_value, 0.2, 1e-7, "calibration ({x}, {y})");
        }
    }
}

#[test]
fn xtrans_direct_and_calibration_black_corrections_match() {
    use crate::io::raw::demosaic::xtrans::XTransImage;
    use crate::io::raw::demosaic::xtrans::internals::test_pattern_array;
    use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;

    let raw_width = 11;
    let raw_height = 11;
    let raw_pattern = test_pattern_array();
    let common_black = 100.0;
    let channel_black = [110.0, 120.0, 130.0];
    let span = 1000.0;
    let raw_data = vec![600u16; raw_width * raw_height];
    let repeat = BlackRepeat {
        size: Size2us::new(3, 2),
        delta_norm: [0.0, 0.002, 0.004, 0.006, 0.008, 0.010].into(),
    };

    for top_margin in 0..6 {
        for left_margin in 0..6 {
            let layout = SensorLayout {
                raw: Size2us::new(raw_width, raw_height),
                active: Size2us::new(6, 6),
                margin: Vec2us::new(left_margin, top_margin),
            };
            let visible_pattern = array::from_fn(|y| {
                array::from_fn(|x| raw_pattern[(y + top_margin) % 6][(x + left_margin) % 6])
            });
            let visible_pattern = XTransPattern::new(visible_pattern).unwrap();
            let active_cfa = CfaType::XTrans(visible_pattern);
            let direct = XTransImage::with_margins(
                &raw_data,
                layout,
                XTransPattern::new(raw_pattern).unwrap(),
                XTransNormalization {
                    channel_black,
                    span,
                    black_repeat: Some(&repeat),
                },
            );
            let calibration = normalize_active_area::<false>(
                &raw_data,
                layout,
                common_black,
                span,
                Some(ChannelBlackDelta::XTrans {
                    visible_pattern,
                    values: [0.01, 0.02, 0.03],
                }),
                Some(&repeat),
            );

            for y in 0..layout.active.height {
                for x in 0..layout.active.width {
                    let raw_y = y + layout.margin.y;
                    let raw_x = x + layout.margin.x;
                    let raw_channel = raw_pattern[raw_y % 6][raw_x % 6] as usize;
                    let visible_channel = visible_pattern.color_at(Vec2us::new(x, y)) as usize;
                    let active_channel = active_cfa.color_at(Vec2us::new(x, y)) as usize;
                    assert_eq!(raw_channel, visible_channel);
                    assert_eq!(raw_channel, active_channel);

                    let expected = [0.49, 0.48, 0.47][raw_channel] - repeat.at_visible(y, x);
                    let direct_value = direct.read_normalized(raw_y, raw_x);
                    let calibration_value = calibration[y * layout.active.width + x];
                    assert_close!(
                        direct_value,
                        expected,
                        1e-7,
                        "direct margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                    assert_close!(
                        calibration_value,
                        expected,
                        1e-7,
                        "calibration margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                }
            }
        }
    }
}

#[cfg(feature = "real-data")]
#[test]
fn real_xtrans_channel_black_matches_direct_and_calibration_paths() {
    use crate::internals::real_data::raw_frames;
    use crate::io::raw::demosaic::xtrans::XTransImage;

    let paths = raw_frames("Lights");
    let Some(raw) = paths
        .iter()
        .filter_map(|path| open_raw(path).ok())
        .find(|raw| {
            matches!(raw.cfa_type, Some(CfaType::XTrans(_)))
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
    let direct = XTransImage::with_margins(
        raw_data,
        raw.layout,
        raw.raw_xtrans_pattern.unwrap(),
        XTransNormalization {
            channel_black: [
                raw.black_level.per_channel[0],
                raw.black_level.per_channel[1],
                raw.black_level.per_channel[2],
            ],
            span: raw.black_level.span,
            black_repeat: raw.black_level.repeat.as_ref(),
        },
    );
    let calibration = raw.extract_cfa_pixels::<false>().unwrap();
    let mut compared = 0usize;

    for y in (0..raw.layout.active.height).step_by(101) {
        for x in (0..raw.layout.active.width).step_by(113) {
            let raw_y = y + raw.layout.margin.y;
            let raw_x = x + raw.layout.margin.x;
            let calibration_value = calibration[y * raw.layout.active.width + x];
            if (0.0..=1.0).contains(&calibration_value) {
                let direct_value = direct.read_normalized(raw_y, raw_x);
                assert_close!(direct_value, calibration_value, 1e-7);
                compared += 1;
            }
        }
    }
    assert!(compared > 100);
}

#[test]
fn camera_white_balance_is_canonicalized() {
    let bayer = Some(CfaType::Bayer(CfaPattern::Rggb));
    assert_eq!(
        canonical_camera_white_balance(bayer, [4.0, 2.0, 3.0, 2.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(bayer, [2.0, 1.0, 1.5, 0.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(Some(CfaType::XTrans(XTRANS_PATTERN)), [2.0, 1.0, 1.5, 9.0]),
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        canonical_camera_white_balance(Some(CfaType::Mono), [2.0, 1.0, 1.5, 1.0]),
        None
    );
    // A sensor LibRaw processes itself (a linear DNG) still reports its multipliers.
    assert_eq!(
        canonical_camera_white_balance(None, [4.0, 2.0, 3.0, 2.0]),
        Some([2.0, 1.0, 1.5, 1.0])
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
    let cfa_type = Some(CfaType::Bayer(CfaPattern::Rggb));

    for input in invalid {
        assert!(
            canonical_camera_white_balance(cfa_type, input).is_none(),
            "{input:?}"
        );
    }
}

/// Uniform black: all cblack zero, scalar black only.
#[test]
fn consolidate_black_levels_uniform() {
    let cblack = no_black();
    // No per-channel, no spatial pattern
    let bl = consolidate_black_levels(&cblack, 512, 16383, 0x9494_9494).unwrap();

    assert_eq!(bl.common, 512.0);
    assert_eq!(bl.per_channel, [512.0; 4]);
    assert_eq!(bl.channel_delta_norm, [0.0; 4]);
    assert_eq!(bl.span, 16383.0 - 512.0);
}

/// Per-channel cblack[0..3] nonzero, no spatial pattern.
#[test]
fn consolidate_black_levels_per_channel() {
    let mut cblack = no_black();
    cblack[0] = 10; // R
    cblack[1] = 5; // G1
    cblack[2] = 15; // B
    cblack[3] = 5; // G2
    // No spatial pattern (cblack[4]==0, cblack[5]==0)

    let bl = consolidate_black_levels(&cblack, 100, 4096, 0x9494_9494).unwrap();

    // Common minimum across channels is 5, moved to black: 100+5=105
    assert_eq!(bl.common, 105.0);
    // Per-channel: cblack[c]-5 + 105
    assert_eq!(bl.per_channel[0], 110.0); // R: 10-5+105
    assert_eq!(bl.per_channel[1], 105.0); // G1: 5-5+105
    assert_eq!(bl.per_channel[2], 115.0); // B: 15-5+105
    assert_eq!(bl.per_channel[3], 105.0); // G2: 5-5+105

    assert_eq!(bl.span, 4096.0 - 105.0);
    // delta_norm[c] = (per_channel[c] - common) / span, divided once
    assert_eq!(bl.channel_delta_norm[0], 5.0 / 3991.0);
    assert_eq!(bl.channel_delta_norm[1], 0.0);
    assert_eq!(bl.channel_delta_norm[2], 10.0 / 3991.0);
    assert!(bl.channel_delta_norm[3].abs() < 1e-10);
}

/// Bayer 2x2 spatial pattern folded into per-channel values.
#[test]
fn consolidate_black_levels_bayer_2x2_fold() {
    let mut cblack = no_black();
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
    let filters = 0x9494_9494_u32;
    let bl = consolidate_black_levels(&cblack, 200, 16383, filters).unwrap();

    assert_eq!(bl.common, 204.0);
    assert_eq!(bl.per_channel[0], 204.0); // R: 0 + 204
    assert_eq!(bl.per_channel[1], 208.0); // G1: 4 + 204
    assert_eq!(bl.per_channel[2], 216.0); // B: 12 + 204
    assert_eq!(bl.per_channel[3], 212.0); // G2: 8 + 204

    assert_eq!(bl.span, 16383.0 - 204.0);
    assert_eq!(bl.channel_delta_norm[0], 0.0); // R: no delta
    assert_eq!(bl.channel_delta_norm[1], 4.0 / 16179.0); // G1
    assert_eq!(bl.channel_delta_norm[2], 12.0 / 16179.0); // B
    assert_eq!(bl.channel_delta_norm[3], 8.0 / 16179.0); // G2
}

/// X-Trans 1x1 spatial pattern folded into all channels.
#[test]
fn consolidate_black_levels_xtrans_1x1_fold() {
    let mut cblack = no_black();
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
    let cblack = no_black();
    let error = consolidate_black_levels(&cblack, 512, 512, 0x9494_9494).unwrap_err();
    assert!(matches!(
        error,
        BlackLevelError::BlackExceedsMaximum {
            black: 512,
            maximum: 512
        }
    ));

    let mut oversized = no_black();
    oversized[4] = 64;
    oversized[5] = 65;
    let error = consolidate_black_levels(&oversized, 0, 4096, 0x9494_9494).unwrap_err();
    assert!(matches!(
        error,
        BlackLevelError::SpatialPatternTooLarge {
            width: 65,
            height: 64,
            capacity: 4098
        }
    ));
}

#[test]
fn apply_bayer_black_corrections_identity() {
    let mut data = vec![0.5f32; 4];
    let delta = [0.0; 4];

    apply_bayer_black_corrections(&mut data, 2, Vec2us::ZERO, 0x9494_9494, &delta, None);

    // No change expected
    for &v in &data {
        assert_close!(v, 0.5, 1e-6);
    }
}

#[test]
fn bayer_black_corrections_apply_a_delta_per_colour() {
    // 2x2 RGGB: positions (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    let mut data = vec![0.5f32; 4];
    let delta = [0.1, 0.0, 0.05, 0.0]; // R has delta=0.1, B has delta=0.05

    apply_bayer_black_corrections(&mut data, 2, Vec2us::ZERO, 0x9494_9494, &delta, None);

    assert_close!(data[0], 0.4, 1e-6, "R: 0.5-0.1=0.4, got {}", data[0]);
    assert_close!(data[1], 0.5, 1e-6, "G: no delta, got {}", data[1]);
    assert_close!(data[2], 0.5, 1e-6, "G: no delta, got {}", data[2]);
    assert_close!(data[3], 0.45, 1e-6, "B: 0.5-0.05=0.45, got {}", data[3]);
}

#[test]
fn apply_bayer_black_corrections_clamp_negative() {
    let mut data = vec![0.05f32; 4];
    let delta = [0.1, 0.0, 0.0, 0.0]; // R delta bigger than value

    apply_bayer_black_corrections(&mut data, 2, Vec2us::ZERO, 0x9494_9494, &delta, None);

    // R at (0,0): (0.05 - 0.1).max(0.0) = 0.0
    assert_eq!(data[0], 0.0, "Should clamp to 0.0");
}

/// libraw's `color.cblack`, every entry zero, on the heap: 16 KiB is too large for the stack.
fn no_black() -> Box<[u32; 4104]> {
    vec![0; 4104]
        .into_boxed_slice()
        .try_into()
        .expect("4104 entries")
}
