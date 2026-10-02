//! Tests for Bayer CFA types and RCD demosaicing.

use crate::io::raw::demosaic::bayer::rcd::INTERPOLATED_BORDER;
use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern, rcd};
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::testing::prelude::*;
use rayon::ThreadPoolBuilder;

/// Every phase round-trips through its `BAYERPAT` spelling, in any case and with blanks around
/// it; `TRUE`, which names no phase, is refused like any other value.
#[test]
fn from_bayerpat_reads_each_spelling_and_refuses_the_rest() {
    for pattern in CfaPattern::ALL {
        let name = pattern.bayerpat();
        assert_eq!(CfaPattern::from_bayerpat(name), Some(pattern));
        assert_eq!(
            CfaPattern::from_bayerpat(&format!(" {} ", name.to_ascii_lowercase())),
            Some(pattern)
        );
    }
    assert_eq!(
        CfaPattern::ALL.map(CfaPattern::bayerpat),
        ["RGGB", "BGGR", "GRBG", "GBRG"]
    );
    for refused in ["TRUE", "XXXX", ""] {
        assert_eq!(CfaPattern::from_bayerpat(refused), None, "{refused:?}");
    }
}

#[test]
fn from_filters_decodes_the_libraw_word_and_rejects_the_rest() {
    // Standard LibRaw encodings: two bits per position, the 2x2 block repeated across the word.
    assert_eq!(
        CfaPattern::from_filters(0x9494_9494),
        Some(CfaPattern::Rggb)
    );
    assert_eq!(
        CfaPattern::from_filters(0x1616_1616),
        Some(CfaPattern::Bggr)
    );
    assert_eq!(
        CfaPattern::from_filters(0x6161_6161),
        Some(CfaPattern::Grbg)
    );
    assert_eq!(
        CfaPattern::from_filters(0x4949_4949),
        Some(CfaPattern::Gbrg)
    );
    // filters == 0 is monochrome / no CFA; exotic patterns match nothing either.
    assert_eq!(CfaPattern::from_filters(0), None);
    assert_eq!(CfaPattern::from_filters(0x1234_5678), None);
    // RGGB in rows 0–5 but BGGR in rows 6–7: not 2-row periodic, whatever the first block says.
    assert_eq!(CfaPattern::from_filters(0x1694_9494), None);
}

#[test]
fn flip_vertical_swaps_rows_and_is_its_own_inverse() {
    // Flip swaps rows: RGGB row0=[R,G] row1=[G,B] → row0=[G,B] row1=[R,G] = GBRG
    assert_eq!(CfaPattern::Rggb.flip_vertical(), CfaPattern::Gbrg);
    assert_eq!(CfaPattern::Gbrg.flip_vertical(), CfaPattern::Rggb);
    assert_eq!(CfaPattern::Bggr.flip_vertical(), CfaPattern::Grbg);
    assert_eq!(CfaPattern::Grbg.flip_vertical(), CfaPattern::Bggr);
    // Double flip is identity
    assert_eq!(
        CfaPattern::Rggb.flip_vertical().flip_vertical(),
        CfaPattern::Rggb
    );
}

#[test]
fn flip_horizontal_swaps_columns_and_is_its_own_inverse() {
    // Flip swaps columns: RGGB row0=[R,G] row1=[G,B] → row0=[G,R] row1=[B,G] = GRBG
    assert_eq!(CfaPattern::Rggb.flip_horizontal(), CfaPattern::Grbg);
    assert_eq!(CfaPattern::Grbg.flip_horizontal(), CfaPattern::Rggb);
    assert_eq!(CfaPattern::Bggr.flip_horizontal(), CfaPattern::Gbrg);
    assert_eq!(CfaPattern::Gbrg.flip_horizontal(), CfaPattern::Bggr);
    // Double flip is identity
    assert_eq!(
        CfaPattern::Rggb.flip_horizontal().flip_horizontal(),
        CfaPattern::Rggb
    );
}

#[test]
fn flip_both_axes() {
    // Flipping both axes is equivalent to 180° rotation
    // RGGB → flip_v → GBRG → flip_h → BGGR
    assert_eq!(
        CfaPattern::Rggb.flip_vertical().flip_horizontal(),
        CfaPattern::Bggr
    );
    assert_eq!(
        CfaPattern::Bggr.flip_vertical().flip_horizontal(),
        CfaPattern::Rggb
    );
}

#[test]
fn raw_origin_pattern_preserves_visible_color_for_every_margin_phase() {
    let visible_patterns = [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ];

    for visible in visible_patterns {
        for top_margin in 0..2 {
            for left_margin in 0..2 {
                let raw = visible.at_raw_origin(top_margin, left_margin);
                for y in 0..4 {
                    for x in 0..4 {
                        assert_eq!(
                            raw.color_at(Vec2us::new(x + left_margin, y + top_margin)),
                            visible.color_at(Vec2us::new(x, y)),
                            "{visible:?}, margin ({top_margin}, {left_margin}), ({y}, {x})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn bayer_image_zero_width() {
    let data = vec![0.0f32; 4];
    let layout = SensorLayout {
        raw: Size2us::new(2, 2),
        active: Size2us::new(0, 2),
        margin: Vec2us::ZERO,
    };
    BayerImage::with_margins(&data, layout, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn bayer_image_zero_height() {
    let data = vec![0.0f32; 4];
    let layout = SensorLayout {
        raw: Size2us::new(2, 2),
        active: Size2us::new(2, 0),
        margin: Vec2us::ZERO,
    };
    BayerImage::with_margins(&data, layout, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Data length")]
fn bayer_image_wrong_data_length() {
    let data = vec![0.0f32; 3];
    let size = Size2us::new(2, 2);
    BayerImage::with_margins(&data, SensorLayout::cropped(size), CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Top margin")]
fn bayer_image_margin_exceeds_height() {
    let data = vec![0.0f32; 4];
    let size = Size2us::new(2, 2);
    let layout = SensorLayout {
        raw: size,
        active: size,
        margin: Vec2us::new(0, 1),
    };
    BayerImage::with_margins(&data, layout, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Left margin")]
fn bayer_image_margin_exceeds_width() {
    let data = vec![0.0f32; 4];
    let size = Size2us::new(2, 2);
    let layout = SensorLayout {
        raw: size,
        active: size,
        margin: Vec2us::new(1, 0),
    };
    BayerImage::with_margins(&data, layout, CfaPattern::Rggb);
}

#[test]
fn bayer_image_valid() {
    let data = vec![0.0f32; 16];
    let layout = SensorLayout {
        raw: Size2us::new(4, 4),
        active: Size2us::new(2, 2),
        margin: Vec2us::new(1, 1),
    };
    let bayer = BayerImage::with_margins(&data, layout, CfaPattern::Rggb);
    assert_eq!(bayer.layout.raw, Size2us::new(4, 4));
    assert_eq!(bayer.layout.active, Size2us::new(2, 2));
    assert_eq!(bayer.layout.margin, Vec2us::new(1, 1));
}

/// Helper: create a `BayerImage` from a flat CFA array with no margins.
fn make_bayer(data: &[f32], size: Size2us, cfa: CfaPattern) -> BayerImage<'_> {
    BayerImage::with_margins(data, SensorLayout::cropped(size), cfa)
}

#[test]
fn cancelled_token_bails_the_demosaic() {
    let size = Size2us::new(20, 20);
    let data = vec![0.5f32; size.pixel_count()];
    let bayer = make_bayer(&data, size, CfaPattern::Rggb);

    // A live, tripped token bails at the first between-stage check rather than
    // running the whole demosaic.
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(
        rcd::demosaic(&bayer, &cancel).is_err(),
        "a tripped cancel token must abort the demosaic"
    );

    // An un-cancelled token completes normally.
    assert!(rcd::demosaic(&bayer, &CancelToken::never()).is_ok());
}

#[test]
fn parallel_rcd_matches_single_thread_bit_for_bit() {
    let size = if cfg!(miri) {
        Size2us::new(20, 20)
    } else {
        Size2us::new(96, 80)
    };
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|index| ((index * 37 + index / size.width * 11) % 1_024) as f32 / 1_023.0)
        .collect();
    let bayer = make_bayer(&data, size, CfaPattern::Rggb);

    let run = |threads| {
        ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rcd::demosaic(&bayer, &CancelToken::never()).unwrap())
    };

    let parallel_threads = if cfg!(miri) { 2 } else { 4 };
    assert_eq!(run(1), run(parallel_threads));
}

/// Demosaic `data` laid out as `size` under `pattern`, with no margins.
fn demosaic(data: &[f32], size: Size2us, pattern: CfaPattern) -> [Vec<f32>; 3] {
    rcd::demosaic(&make_bayer(data, size, pattern), &CancelToken::never()).unwrap()
}

/// A mosaic of `size` under `pattern` sampling each pixel's own colour of `scene`.
fn mosaic(size: Size2us, pattern: CfaPattern, scene: impl Fn(Vec2us, usize) -> f32) -> Vec<f32> {
    (0..size.pixel_count())
        .map(|index| {
            let pos = size.point_of(index);
            scene(pos, pattern.color_at(pos))
        })
        .collect()
}

/// A constant colour per channel — uniform grey and two distinct (R, G, B) — through every phase,
/// with and without masked margins, comes back as itself at every pixel, the interpolated border
/// included, and each native sample exactly.
///
/// The ratio correction scales by `2·lpf / (EPS + 2·lpf)`, off by `EPS / (2·lpf)` relative; these
/// colours put every low-pass value at 2 or above, so each of the two chained corrections is off
/// by at most 2.5e-6 of a value below 1, plus a few f32 roundings: under 5e-6 together. The border
/// averages equal neighbours and is exact up to rounding.
#[test]
fn constant_colour_reconstructs_on_every_phase_and_margin() {
    let active = Size2us::new(40, 36);
    for pattern in CfaPattern::ALL {
        for colour in [[0.5; 3], [0.8, 0.5, 0.2], [0.1, 0.9, 0.4]] {
            for margin in [Vec2us::ZERO, Vec2us::new(5, 4), Vec2us::new(4, 5)] {
                let raw = Size2us::new(active.width + 2 * margin.x, active.height + 2 * margin.y);
                let raw_pattern = pattern.at_raw_origin(margin.y, margin.x);
                let data = mosaic(raw, raw_pattern, |_, channel| colour[channel]);
                let layout = SensorLayout {
                    raw,
                    active,
                    margin,
                };
                let planes = rcd::demosaic(
                    &BayerImage::with_margins(&data, layout, raw_pattern),
                    &CancelToken::never(),
                )
                .unwrap();
                for (index, pos) in (0..active.pixel_count()).map(|i| (i, active.point_of(i))) {
                    let native = pattern.color_at(pos);
                    assert_eq!(planes[native][index].to_bits(), colour[native].to_bits());
                    for channel in 0..3 {
                        let value = planes[channel][index];
                        assert_close!(
                            value,
                            colour[channel],
                            5e-6,
                            "{pattern:?} {colour:?} margin {margin:?} channel {channel} at {pos:?}: {value}"
                        );
                    }
                }
            }
        }
    }
}

/// From [`INTERPOLATED_BORDER`] in, a frame demosaics bit for bit as the same pixels inside a
/// larger frame: no stage reads a value it did not compute from the frame's own samples. Random
/// samples, so no stencil can hide behind equal neighbours.
#[test]
fn rcd_beyond_the_border_matches_a_larger_frame() {
    let large = Size2us::new(96, 96);
    let offset = 16;
    let mut rng = TestRng::new(7);
    let samples: Vec<f32> = (0..large.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    let small = Size2us::new(64, 64);
    let crop: Vec<f32> = (0..small.pixel_count())
        .map(|index| {
            let pos = small.point_of(index);
            samples[large.index_of(Vec2us::new(pos.x + offset, pos.y + offset))]
        })
        .collect();
    for pattern in CfaPattern::ALL {
        let whole = demosaic(&samples, large, pattern);
        let part = demosaic(&crop, small, pattern.at_raw_origin(offset, offset));
        for (channel, (part_plane, whole_plane)) in part.iter().zip(&whole).enumerate() {
            for (index, value) in part_plane.iter().enumerate() {
                let pos = small.point_of(index);
                let distance = pos
                    .x
                    .min(pos.y)
                    .min(small.width - 1 - pos.x)
                    .min(small.height - 1 - pos.y);
                if distance < INTERPOLATED_BORDER {
                    continue;
                }
                let outer = large.index_of(Vec2us::new(pos.x + offset, pos.y + offset));
                assert_eq!(
                    value.to_bits(),
                    whole_plane[outer].to_bits(),
                    "channel {channel} at {pos:?}"
                );
            }
        }
    }
}

#[test]
fn rcd_all_patterns_preserve_native_samples_and_stay_finite() {
    let size = Size2us::new(20, 20);
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|i| i as f32 / size.pixel_count() as f32)
        .collect();

    for pattern in CfaPattern::ALL {
        let planes = demosaic(&data, size, pattern);
        for (channel, plane) in planes.iter().enumerate() {
            for (index, &value) in plane.iter().enumerate() {
                assert!(
                    value.is_finite(),
                    "{pattern:?} channel {channel} at {index} = {value}"
                );
            }
        }
        for index in 0..size.pixel_count() {
            let channel = pattern.color_at(size.point_of(index));
            assert_eq!(
                planes[channel][index], data[index],
                "{pattern:?}: native sample changed at {index}"
            );
        }
    }
}

/// A signed horizontal ramp through zero comes back as the ramp in every channel. Inside the
/// border the ratio correction blends to its additive form where the low-pass sum cancels, which
/// keeps the error at f32 rounding of the ramp's slope sums (2e-4 on values up to 2); the
/// border averages symmetric neighbours of a linear ramp, which is exact.
#[test]
fn signed_linear_gradient_crossing_zero_is_reconstructed_without_spikes() {
    let size = Size2us::new(32, 24);
    let slope = 0.125;
    for pattern in CfaPattern::ALL {
        let data = mosaic(size, pattern, |pos, _| (pos.x as f32 - 15.5) * slope);
        let planes = demosaic(&data, size, pattern);
        for y in 1..size.height - 1 {
            for x in 1..size.width - 1 {
                let expected = (x as f32 - 15.5) * slope;
                for (channel, plane) in planes.iter().enumerate() {
                    let actual = plane[y * size.width + x];
                    assert_close!(
                        actual,
                        expected,
                        2e-4,
                        "{pattern:?} channel {channel} at ({x}, {y}): expected {expected}, got {actual}"
                    );
                }
            }
        }
    }
}

/// RCD is translation invariant: a Bayer phase is the RGGB mosaic shifted by a column, a row, or
/// both, so demosaicing the shifted crop under its own pattern must reproduce the RGGB result at
/// every pixel away from the borders, bit for bit.
///
/// The scene has diagonal structure, so the P/Q (diagonal) direction choice matters at every
/// red and blue site; a flat field or a horizontal ramp would make P and Q equal and hide a
/// direction filter computed at the wrong sites.
#[test]
fn rcd_is_the_same_on_every_bayer_phase() {
    let (w, h) = (48, 40);
    let scene = |x: usize, y: usize, c: usize| -> f32 {
        let (x, y) = (x as f32, y as f32);
        let diagonal = if x + y > 40.0 { 0.35 } else { 0.0 };
        let anti = if x - y > 6.0 { 0.2 } else { 0.0 };
        match c {
            0 => 0.2 + diagonal + 0.15 * (0.7 * x + 0.3 * y).sin(),
            1 => 0.3 + anti + 0.15 * (0.5 * x - 0.6 * y).sin(),
            _ => 0.25 + diagonal - anti + 0.1 * (0.4 * x + 0.9 * y).cos(),
        }
    };
    let mosaic = |pattern: CfaPattern, dx: usize, dy: usize| -> Vec<f32> {
        let (cw, ch) = (w - dx, h - dy);
        (0..cw * ch)
            .map(|i| {
                let (x, y) = (i % cw, i / cw);
                scene(x + dx, y + dy, pattern.color_at(Vec2us::new(x, y)))
            })
            .collect()
    };

    let base_data = mosaic(CfaPattern::Rggb, 0, 0);
    let base = rcd::demosaic(
        &make_bayer(&base_data, Size2us::new(w, h), CfaPattern::Rggb),
        &CancelToken::never(),
    )
    .unwrap();

    // Each image interpolates its own outer `INTERPOLATED_BORDER` pixels, and a crop shifted by one
    // has its band one pixel further in; past that, both images are RCD's own.
    const MARGIN: usize = INTERPOLATED_BORDER + 1;
    for (pattern, dx, dy) in [
        (CfaPattern::Grbg, 1, 0),
        (CfaPattern::Gbrg, 0, 1),
        (CfaPattern::Bggr, 1, 1),
    ] {
        assert_eq!(
            pattern.color_at(Vec2us::new(0, 0)),
            CfaPattern::Rggb.color_at(Vec2us::new(dx, dy)),
            "{pattern:?} is RGGB shifted by ({dx}, {dy})"
        );
        let (cw, ch) = (w - dx, h - dy);
        let data = mosaic(pattern, dx, dy);
        let shifted = rcd::demosaic(
            &make_bayer(&data, Size2us::new(cw, ch), pattern),
            &CancelToken::never(),
        )
        .unwrap();
        for y in MARGIN..ch - MARGIN {
            for x in MARGIN..cw - MARGIN {
                for channel in 0..3 {
                    let got = shifted[channel][y * cw + x];
                    let want = base[channel][(y + dy) * w + x + dx];
                    assert_eq!(
                        got.to_bits(),
                        want.to_bits(),
                        "{pattern:?} channel {channel} at ({x}, {y}): {got} vs RGGB {want}"
                    );
                }
            }
        }
    }
}

/// RCD interpolates along an edge, not across it. Rows on both sides of a horizontal edge, and
/// columns on both sides of a vertical one, keep their side's value in green to the ratio
/// correction's EPS-level error, where interpolating across the edge — as bilinear does — would
/// mix in a quarter of the other side: 0.15 here.
#[test]
fn rcd_interpolates_along_an_edge() {
    fn side(coordinate: usize) -> f32 {
        if coordinate < 16 { 0.8 } else { 0.2 }
    }
    let size = Size2us::new(32, 32);
    for (name, scene) in [
        (
            "horizontal",
            (|pos: Vec2us, _| side(pos.y)) as fn(Vec2us, usize) -> f32,
        ),
        ("vertical", |pos: Vec2us, _| side(pos.x)),
    ] {
        let data = mosaic(size, CfaPattern::Rggb, scene);
        let green = &demosaic(&data, size, CfaPattern::Rggb)[1];
        for edge in [15, 16] {
            for along in INTERPOLATED_BORDER..size.width - INTERPOLATED_BORDER {
                let pos = if name == "horizontal" {
                    Vec2us::new(along, edge)
                } else {
                    Vec2us::new(edge, along)
                };
                let value = green[size.index_of(pos)];
                assert_close!(
                    value,
                    side(edge),
                    1e-5,
                    "{name} edge, green at {pos:?}: {value}"
                );
            }
        }
    }
}

#[test]
fn rcd_gradient_image_green_smoothness() {
    // A horizontal gradient should produce a smooth green channel.
    // No abrupt jumps between adjacent green values in the interior.
    let size = Size2us::new(32, 16);
    let data = mosaic(size, CfaPattern::Rggb, |pos, _| {
        0.1 + 0.8 * (pos.x as f32 / (size.width - 1) as f32)
    });
    let green = &demosaic(&data, size, CfaPattern::Rggb)[1];

    let border = 5;
    for y in border..size.height - border {
        let mut prev_g = 0.0f32;
        for x in border..size.width - border {
            let g = green[y * size.width + x];
            if x > border {
                assert!(
                    g > prev_g - 0.05,
                    "Green not monotonic at ({x},{y}): {g} < {prev_g} - 0.05"
                );
            }
            prev_g = g;
        }
    }
}

#[test]
fn rcd_sharp_edge_no_excessive_artifacts() {
    // A sharp vertical edge at column 16: left half = 0.9, right half = 0.1.
    // Verify that the transition zone is bounded (no extreme overshoots from
    // the ratio correction or direction interpolation).
    let size = Size2us::new(32, 32);
    let data = mosaic(
        size,
        CfaPattern::Rggb,
        |pos, _| if pos.x < 16 { 0.9 } else { 0.1 },
    );
    let planes = demosaic(&data, size, CfaPattern::Rggb);
    for plane in &planes {
        for (i, &val) in plane.iter().enumerate() {
            assert!(val.is_finite(), "pixel {i} is non-finite: {val}");
        }
    }

    let border = 5;
    for y in border..size.height - border {
        for (range, check) in [
            (6..11, (|v: f32| v > 0.7) as fn(f32) -> bool),
            (21..26, |v: f32| v < 0.3),
        ] {
            for x in range {
                for (c, plane) in planes.iter().enumerate() {
                    let value = plane[y * size.width + x];
                    assert!(check(value), "({x},{y}) ch {c}={value}");
                }
            }
        }
        let mut prev = 1.0f32;
        for x in 13..20 {
            let g = planes[1][y * size.width + x];
            assert!(
                g < prev + 0.15,
                "Edge transition not bounded at ({x},{y}): g={g}, prev={prev}"
            );
            prev = g;
        }
    }
}
