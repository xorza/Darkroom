use crate::internals::prelude::*;
use crate::io::raw::demosaic::xtrans::internals::{make_xtrans, test_pattern};
use crate::io::raw::demosaic::xtrans::markesteijn::*;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;

const PASSES: [MarkesteijnPasses; 2] = [MarkesteijnPasses::One, MarkesteijnPasses::Three];

/// The interior of every scene, at one pass and at three, is librtprocess's Markesteijn to the bit.
///
/// `internals/reference/markesteijn_librtprocess.py` builds librtprocess's `markesteijn.cc` at a
/// pinned commit, on its scalar paths and with YPbPr at both pass counts, and prints each case's
/// FNV-1a 64 digest of the output inside the pass count's border: the planes in order, then rows,
/// then columns, each f32's little-endian bytes. The script changes two lines: the 2×2 green
/// blocks take red and blue in all four of one pass's directions, not two, and one tile covers the
/// frame, whose seams would differ from an untiled run. The 160×120 frame spans two of lumos's
/// tiles, so their seam is in the digest. The scenes use only correctly rounded operations, so
/// their samples are the same bits on every platform. The two pass counts give different digests,
/// so the count reaches the output.
#[test]
fn markesteijn_matches_librtprocess_bit_for_bit() {
    /// A scene's sample of `channel` at `(x, y)`.
    type Scene = fn(usize, usize, usize) -> f32;
    const WIDTH: usize = 160;
    const HEIGHT: usize = 120;
    const DIGESTS: [[u64; 4]; 2] = [
        [
            0x6ce19c3e3aaefce5,
            0xbe8c9f31a4aed927,
            0x67eea713c538876b,
            0x7a207fa29d1a84e5,
        ],
        [
            0xb2fecff3b5416a2d,
            0x949566b3f3644e3e,
            0x261009dc275379eb,
            0x9cefbe2bac38b375,
        ],
    ];
    let scenes: [(&str, Scene); 4] = [
        ("colour edge", |channel, x, _| {
            let (left, right) = ([0.1, 0.3, 0.8], [0.9, 0.6, 0.2]);
            if x < WIDTH / 2 {
                left[channel]
            } else {
                right[channel]
            }
        }),
        ("impulse", |channel, x, y| {
            if x == WIDTH / 2 && y == HEIGHT / 2 {
                [1.0, 0.7, 0.4][channel]
            } else {
                0.05
            }
        }),
        ("star", |channel, x, y| {
            let dx = x as f32 - (WIDTH - 1) as f32 * 0.5;
            let dy = y as f32 - (HEIGHT - 1) as f32 * 0.5;
            let width = [1.2f32, 1.6, 2.0][channel];
            let amplitude = [0.9f32, 0.7, 0.5][channel];
            0.02 + amplitude / (1.0 + (dx * dx + dy * dy) / (width * width))
        }),
        ("colour grating", |channel, x, y| {
            let phase = [0.0f32, 0.333_333_34, 0.666_666_7][channel];
            let t = 0.075 * x as f32 + 0.05 * y as f32 + phase;
            0.1 + 1.6 * (t - t.floor() - 0.5).abs()
        }),
    ];
    let size = Size2us::new(WIDTH, HEIGHT);
    let pattern = test_pattern();
    for (passes, digests) in PASSES.into_iter().zip(DIGESTS) {
        for ((name, scene), expected) in scenes.into_iter().zip(digests) {
            let data: Vec<f32> = (0..size.pixel_count())
                .map(|index| {
                    let (x, y) = (index % WIDTH, index / WIDTH);
                    scene(usize::from(pattern.color_at(Vec2us::new(x, y))), x, y)
                })
                .collect();
            let planes = demosaic(
                &XTransImage::new(&data, size, pattern),
                passes,
                &CancelToken::never(),
            )
            .unwrap();
            let border = passes.border();
            let mut digest = 0xcbf2_9ce4_8422_2325u64;
            for plane in &planes {
                for row in border..HEIGHT - border {
                    let interior = &plane[row * WIDTH + border..(row + 1) * WIDTH - border];
                    for byte in interior.iter().flat_map(|value| value.to_le_bytes()) {
                        digest = (digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
                    }
                }
            }
            assert_eq!(digest, expected, "{name}, {passes:?}: {digest:#018x}");
        }
    }
}

/// A constant colour per channel — uniform grey and two distinct (R, G, B) — comes back as itself
/// at every pixel, border included, at every frame size: 12 and 24 have no tile and are all border
/// fill, 25 one tile cut short by the frame at one pass and none at three, 37 one at both, 130 two
/// tiles each way. The output starts as
/// zeros, so a pixel nobody writes fails. Every stage averages or blends equal values, so each
/// output is the input to a few f32 roundings: under 2e-7, where one rounding of a value below 1 is
/// up to 6e-8.
#[test]
fn constant_colour_reconstructs_to_rounding_at_every_size() {
    let pattern = test_pattern();
    for side in [12, 24, 25, 37, 130] {
        let size = Size2us::new(side, side);
        for colour in [[0.5f32; 3], [0.8, 0.5, 0.2], [0.1, 0.9, 0.4]] {
            let data: Vec<f32> = (0..size.pixel_count())
                .map(|index| colour[usize::from(pattern.color_at(size.point_of(index)))])
                .collect();
            for passes in PASSES {
                let planes = demosaic(
                    &XTransImage::new(&data, size, pattern),
                    passes,
                    &CancelToken::never(),
                )
                .unwrap();
                for (channel, plane) in planes.iter().enumerate() {
                    for (index, &value) in plane.iter().enumerate() {
                        assert_close!(
                            value,
                            colour[channel],
                            2e-7,
                            "{side}, {colour:?}, {passes:?}, channel {channel} at {index}: {value}"
                        );
                    }
                }
            }
        }
    }
}

/// A ramp stays finite, and all zeros stay zero exactly: no stage divides by a sum of differences
/// that a flat field makes zero.
#[test]
fn markesteijn_ramp_is_finite_and_zeros_stay_zero() {
    let size = Size2us::new(40, 40);
    let ramp: Vec<f32> = (0..size.pixel_count())
        .map(|i| i as f32 / size.pixel_count() as f32)
        .collect();
    let zeros = vec![0.0f32; size.pixel_count()];
    for passes in PASSES {
        let planes = demosaic(&make_xtrans(&ramp, size), passes, &CancelToken::never()).unwrap();
        for (i, &v) in planes.iter().flatten().enumerate() {
            assert!(v.is_finite(), "{passes:?}: {v} at {i}");
        }
        let planes = demosaic(&make_xtrans(&zeros, size), passes, &CancelToken::never()).unwrap();
        for &v in planes.iter().flatten() {
            assert_eq!(v.to_bits(), 0, "{passes:?}");
        }
    }
}

/// Each pass count's margin is the least that keeps every pixel a tile writes clear of what its
/// passes leave uncomputed.
///
/// The tile runs twice, with every channel no sample gives seeded at +1000 and at −1000: a cell
/// whose colours differ reads, through some chain of stages, a value no stage computed. A pixel's
/// output reads the colours of every direction within 4 of it — the second differences reach 1, the
/// 3×3 homogeneity counts 1 more, their 5×5 sums 2 more — so a margin `m` holds when no such cell
/// lies within 4 of the part `[m, M − m)` a tile of extent `M` writes. The reads repeat every three
/// rows and columns, so three tiles, starting at each phase and cut short by the frame at each,
/// meet every case at both edges. Random samples, so opposite colours take both of their axes
/// often at every phase.
#[test]
fn margin_is_the_least_that_reads_only_computed_colours() {
    const POISON: f32 = 1000.0;
    const READ_REACH: usize = 4;
    let pattern = test_pattern();
    let mut rng = TestRng::new(5);
    for passes in PASSES {
        let mut least = 0;
        for phase in 0..3 {
            let place = TilePlace {
                top: 3 + phase,
                left: 3 + phase,
            };
            let extent = Size2us::new(113, 111);
            let size = Size2us::new(place.left + extent.width + 3, place.top + extent.height + 3);
            let data: Vec<f32> = (0..size.pixel_count())
                .map(|_| 0.1 + 0.8 * rng.next_f32())
                .collect();
            let xtrans = XTransImage::new(&data, size, pattern);
            let hex = HexTable::new(pattern, size.width);
            let mut tile = Tile::new(passes.directions());
            let high = tile
                .interpolate_poisoned(&xtrans, &hex, place, passes.count(), POISON)
                .to_vec();
            let low = tile.interpolate_poisoned(&xtrans, &hex, place, passes.count(), -POISON);
            let plane = TILE * TILE;
            for (index, (a, b)) in high
                .iter()
                .zip(low)
                .enumerate()
                .take(passes.directions() * plane)
            {
                if a == b {
                    continue;
                }
                let (row, col) = ((index % plane) / TILE, index % TILE);
                // Below `row + READ_REACH + 1`, above `M − row + READ_REACH`, or likewise for the
                // column, the written part keeps clear of the cell.
                let clear = (row + READ_REACH + 1)
                    .min((extent.height + READ_REACH).saturating_sub(row))
                    .min(col + READ_REACH + 1)
                    .min((extent.width + READ_REACH).saturating_sub(col));
                least = least.max(clear);
            }
        }
        assert_eq!(least, passes.margin(), "{passes:?}");
    }
}

/// Inside its border, every frame demosaics bit for bit as the same pixels inside a larger frame,
/// though its tiles lie elsewhere on the pixels: the tiles cover the frame, no two write a pixel,
/// and a seam changes nothing. The crops start at three offsets, with the 6×6 layout shifted to
/// match, and their sizes vary, so the tiles' edges and the frame's meet each phase of the reads.
/// Random samples, so no stencil can hide behind equal neighbours.
#[test]
fn markesteijn_inside_the_border_matches_a_larger_frame() {
    let large = Size2us::new(152, 144);
    let mut rng = TestRng::new(7);
    let samples: Vec<f32> = (0..large.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    let rows = *test_pattern().rows();
    for passes in PASSES {
        let whole = demosaic(
            &XTransImage::new(&samples, large, test_pattern()),
            passes,
            &CancelToken::never(),
        )
        .unwrap();
        for (ox, oy) in [(12, 12), (13, 14), (14, 13)] {
            let small = Size2us::new(128 + (ox + 2 * oy) % 3, 120 + (2 * ox + oy) % 3);
            let mut shifted = [[0u8; 6]; 6];
            for (y, row) in shifted.iter_mut().enumerate() {
                for (x, colour) in row.iter_mut().enumerate() {
                    *colour = rows[(y + oy) % 6][(x + ox) % 6];
                }
            }
            let crop: Vec<f32> = (0..small.pixel_count())
                .map(|index| {
                    let pos = small.point_of(index);
                    samples[large.index_of(Vec2us::new(pos.x + ox, pos.y + oy))]
                })
                .collect();
            let pattern = XTransPattern::new(shifted).unwrap();
            let part = demosaic(
                &XTransImage::new(&crop, small, pattern),
                passes,
                &CancelToken::never(),
            )
            .unwrap();
            let border = passes.border();
            for (channel, (part_plane, whole_plane)) in part.iter().zip(&whole).enumerate() {
                for y in border..small.height - border {
                    for x in border..small.width - border {
                        let value = part_plane[small.index_of(Vec2us::new(x, y))];
                        let outer = whole_plane[large.index_of(Vec2us::new(x + ox, y + oy))];
                        assert_eq!(
                            value.to_bits(),
                            outer.to_bits(),
                            "{passes:?}, offset ({ox}, {oy}), channel {channel} at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }
}
