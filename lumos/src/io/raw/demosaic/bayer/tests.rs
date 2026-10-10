//! Tests for Bayer CFA types and RCD demosaicing.

use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::prelude::*;
use crate::io::image::cfa::CfaType;
use crate::io::raw::demosaic::bayer::rcd::INTERPOLATED_BORDER;
use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern, rcd};
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
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

/// A frame must hold one sample per pixel of a size that is not empty.
#[test]
fn a_bayer_image_holds_its_size_in_samples() {
    let bayer = BayerImage::new(&[0.0; 6], Size2us::new(3, 2), CfaPattern::Rggb);
    assert_eq!(bayer.size, Size2us::new(3, 2));
    for (samples, size) in [
        (4, Size2us::new(0, 2)),
        (4, Size2us::new(2, 0)),
        (3, Size2us::new(2, 2)),
    ] {
        let data = vec![0.0f32; samples];
        let refused = std::panic::catch_unwind(|| {
            BayerImage::new(&data, size, CfaPattern::Rggb);
        });
        assert!(refused.is_err(), "{samples} samples for {size:?}");
    }
}

fn make_bayer(data: &[f32], size: Size2us, cfa: CfaPattern) -> BayerImage<'_> {
    BayerImage::new(data, size, cfa)
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

/// What a few f32 roundings of each RCD step leave of a value: 2⁻²¹, eight units in its last
/// place. On a field RCD reconstructs exactly in exact arithmetic — a constant, a linear ramp, the
/// two sides of an edge — every output stays within it.
const ROUNDING: f32 = 1.0 / (1 << 21) as f32;

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

/// A constant colour per channel — uniform grey and two distinct (R, G, B) — through every phase
/// comes back as itself at every pixel, the interpolated border included, and each native sample
/// exactly.
///
/// The ratio correction is level-free, so a constant colour comes back to a few f32 roundings of
/// each step: 2⁻²¹ of the value, eight units in the last place, holds them. The border averages
/// equal neighbours and is exact up to rounding.
#[test]
fn constant_colour_reconstructs_on_every_phase() {
    let size = Size2us::new(40, 36);
    for pattern in CfaPattern::ALL {
        for colour in [[0.5; 3], [0.8, 0.5, 0.2], [0.1, 0.9, 0.4]] {
            let data = mosaic(size, pattern, |_, channel| colour[channel]);
            let planes = demosaic(&data, size, pattern);
            for (index, pos) in (0..size.pixel_count()).map(|i| (i, size.point_of(i))) {
                let native = pattern.color_at(pos);
                assert_eq!(planes[native][index].to_bits(), colour[native].to_bits());
                for channel in 0..3 {
                    let value = planes[channel][index];
                    assert_close!(
                        value,
                        colour[channel],
                        colour[channel] * ROUNDING,
                        "{pattern:?} {colour:?} channel {channel} at {pos:?}: {value}"
                    );
                }
            }
        }
    }
}

/// From [`INTERPOLATED_BORDER`] in, a frame demosaics bit for bit as the same pixels inside a
/// larger frame: no stage reads a value it did not compute from the frame's own samples, and the
/// tiles' seams, which lie elsewhere on the pixels in each, change nothing. Both frames span three
/// tiles each way. Random samples, so no stencil can hide behind equal neighbours.
#[test]
fn rcd_beyond_the_border_matches_a_larger_frame() {
    let large = Size2us::new(300, 280);
    let offset = 16;
    let mut rng = TestRng::new(7);
    let samples: Vec<f32> = (0..large.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    let small = Size2us::new(240, 236);
    let crop: Vec<f32> = (0..small.pixel_count())
        .map(|index| {
            let pos = small.point_of(index);
            samples[large.index_of(Vec2us::new(pos.x + offset, pos.y + offset))]
        })
        .collect();
    for pattern in CfaPattern::ALL {
        let whole = demosaic(&samples, large, pattern);
        // An even offset keeps the crop on the larger frame's phase.
        let part = demosaic(&crop, small, pattern);
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

/// Every native sample comes out as it went in, bit for bit, and every output is finite: through
/// RCD in each Bayer phase, and through `CfaImage::demosaic` — RCD and Markesteijn at each pass
/// count — under a camera white balance of red 2.13 and blue 1.71 over green, on a frame of 130
/// that both kernels tile as well as fill at its border. The kernels read the samples balanced and
/// write each native one from the input, so no `(x·g)/g` round trip rounds it, and an
/// interpolated sample is balanced back by its own colour's gain.
#[test]
fn rcd_all_patterns_preserve_native_samples_and_stay_finite() {
    let ramp = |size: Size2us| -> Vec<f32> {
        (0..size.pixel_count())
            .map(|i| i as f32 / size.pixel_count() as f32)
            .collect()
    };
    let assert_native = |planes: &[Vec<f32>; 3],
                         data: &[f32],
                         colour: &dyn Fn(Vec2us) -> usize,
                         size: Size2us,
                         case: &str| {
        for (channel, plane) in planes.iter().enumerate() {
            for (index, &value) in plane.iter().enumerate() {
                assert!(
                    value.is_finite(),
                    "{case} channel {channel} at {index} = {value}"
                );
            }
        }
        for (index, &sample) in data.iter().enumerate() {
            let channel = colour(size.point_of(index));
            assert_eq!(
                planes[channel][index].to_bits(),
                sample.to_bits(),
                "{case}: native sample changed at {index}"
            );
        }
    };
    let size = Size2us::new(20, 20);
    let data = ramp(size);
    for pattern in CfaPattern::ALL {
        assert_native(
            &demosaic(&data, size, pattern),
            &data,
            &|pos| pattern.color_at(pos),
            size,
            &format!("{pattern:?}"),
        );
    }

    let size = Size2us::new(130, 130);
    let data = ramp(size);
    for (cfa_type, passes) in [
        (CfaType::Bayer(CfaPattern::Rggb), MarkesteijnPasses::One),
        (CfaType::Bayer(CfaPattern::Gbrg), MarkesteijnPasses::One),
        (CfaType::XTrans(XTRANS_PATTERN), MarkesteijnPasses::One),
        (CfaType::XTrans(XTRANS_PATTERN), MarkesteijnPasses::Three),
    ] {
        let mut cfa = make_cfa(size, data.clone(), cfa_type);
        cfa.metadata.camera_white_balance = Some([2.13, 1.0, 1.71, 1.0]);
        let image = cfa.demosaic(passes, &CancelToken::never()).unwrap();
        let planes = [0, 1, 2].map(|channel| image.channel(channel).pixels().to_vec());
        assert_native(
            &planes,
            &data,
            &|pos| usize::from(cfa_type.color_at(pos)),
            size,
            &format!("{cfa_type:?} {passes:?}, balanced"),
        );
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
    // Each image interpolates its own outer `INTERPOLATED_BORDER` pixels, and a crop shifted by one
    // has its band one pixel further in; past that, both images are RCD's own.
    const MARGIN: usize = INTERPOLATED_BORDER + 1;

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

/// RCD interpolates along an edge, not across it: inside the border, every pixel of every channel
/// on either side of a horizontal or a vertical edge keeps its side's value to [`ROUNDING`], the
/// two rows or columns at the edge among them, where interpolating across it — as bilinear does —
/// would mix in a quarter of the other side, 0.15 here.
#[test]
fn rcd_interpolates_along_an_edge() {
    fn side(coordinate: usize) -> f32 {
        if coordinate < 16 { 0.8 } else { 0.2 }
    }
    let size = Size2us::new(32, 32);
    for pattern in CfaPattern::ALL {
        for (name, across) in [
            ("horizontal", (|pos: Vec2us| pos.y) as fn(Vec2us) -> usize),
            ("vertical", |pos: Vec2us| pos.x),
        ] {
            let data = mosaic(size, pattern, |pos, _| side(across(pos)));
            let planes = demosaic(&data, size, pattern);
            for index in 0..size.pixel_count() {
                let pos = size.point_of(index);
                if pos
                    .x
                    .min(pos.y)
                    .min(size.width - 1 - pos.x)
                    .min(size.height - 1 - pos.y)
                    < INTERPOLATED_BORDER
                {
                    continue;
                }
                let expected = side(across(pos));
                for (channel, plane) in planes.iter().enumerate() {
                    assert_close!(
                        plane[index],
                        expected,
                        expected * ROUNDING,
                        "{pattern:?} {name} edge, channel {channel} at {pos:?}"
                    );
                }
            }
        }
    }
}

/// A linear ramp `0.1 + 0.015x + 0.01y` comes back as itself inside the border in every channel
/// and phase: each RCD step is exact on a linear field — a ratio `g·2c/(c + s)` of a neighbour one
/// pixel off and low-pass values two apart is `(v ± b)·2v/(2v ± 2b) = v` — so only rounding is
/// left.
#[test]
fn rcd_reproduces_a_linear_ramp_inside_the_border() {
    let size = Size2us::new(48, 40);
    let ramp = |pos: Vec2us| 0.1 + 0.015 * pos.x as f32 + 0.01 * pos.y as f32;
    for pattern in CfaPattern::ALL {
        let planes = demosaic(&mosaic(size, pattern, |pos, _| ramp(pos)), size, pattern);
        for index in 0..size.pixel_count() {
            let pos = size.point_of(index);
            if pos
                .x
                .min(pos.y)
                .min(size.width - 1 - pos.x)
                .min(size.height - 1 - pos.y)
                < INTERPOLATED_BORDER
            {
                continue;
            }
            for (channel, plane) in planes.iter().enumerate() {
                assert_close!(
                    plane[index],
                    ramp(pos),
                    ramp(pos) * ROUNDING,
                    "{pattern:?} channel {channel} at {pos:?}"
                );
            }
        }
    }
}

/// A flat field comes back as its level at every level, the faint and the signed among them: the
/// ratio carries no `eps`, which at 1e-5 would have put green 11% low at red and blue sites, `8v/(eps + 8v)`. Each sample is the level to
/// one unit in its last place.
#[test]
fn rcd_returns_a_flat_field_at_any_level() {
    let size = Size2us::new(32, 24);
    for pattern in CfaPattern::ALL {
        for level in [-1e-4f32, 0.0, 1e-5, 1e-4, 1e-2] {
            let planes = demosaic(&vec![level; size.pixel_count()], size, pattern);
            for (channel, plane) in planes.iter().enumerate() {
                for &value in plane {
                    assert_close!(
                        value,
                        level,
                        level.abs() * f32::EPSILON,
                        "{pattern:?} level {level} channel {channel}"
                    );
                }
            }
        }
    }
}

/// RCD is odd: every step is a ratio, a difference, an absolute value or a square, so the
/// negated mosaic demosaics to the negated output, bit for bit — a signed field, as calibration
/// leaves the background, is treated as its mirror is.
#[test]
fn rcd_is_odd() {
    let size = Size2us::new(40, 32);
    let mut rng = TestRng::new(11);
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.02 * (rng.next_f32() - 0.5))
        .collect();
    let negated: Vec<f32> = data.iter().map(|&value| -value).collect();
    for pattern in CfaPattern::ALL {
        let planes = demosaic(&data, size, pattern);
        let mirrored = demosaic(&negated, size, pattern);
        for (channel, (plane, mirror)) in planes.iter().zip(&mirrored).enumerate() {
            for (index, (&value, &negative)) in plane.iter().zip(mirror).enumerate() {
                assert_eq!(
                    (-value).to_bits(),
                    negative.to_bits(),
                    "{pattern:?} channel {channel} at {index}"
                );
            }
        }
    }
}
