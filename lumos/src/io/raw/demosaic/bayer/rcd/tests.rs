use common::CancelToken;

use crate::internals::test_rng::TestRng;
use crate::io::raw::demosaic::bayer::rcd::tile::Tile;
use crate::io::raw::demosaic::bayer::rcd::{
    INTERPOLATED_BORDER, MIN_SIGNED_DENOMINATOR_RATIO, TILE, demosaic, estimate_green,
};
use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern};
use crate::io::raw::demosaic::tiled::{OutputPlanes, TilePlace};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::simd::portable::Portable;
use crate::simd::{F32_LANES, F32x8, Isa};

/// [`estimate_green`] of one site, on [`Portable`], the model every hardware Isa is held to.
fn estimate(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    let isa = Portable::new();
    let lanes = estimate_green(
        isa,
        isa.splat_f32(neighbor_green),
        isa.splat_f32(center_lpf),
        isa.splat_f32(same_color_lpf),
    )
    .to_array();
    assert!(
        lanes
            .iter()
            .all(|lane| lane.to_bits() == lanes[0].to_bits())
    );
    lanes[0]
}

fn canonical_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    neighbor_green * (center_lpf + center_lpf) / (center_lpf + same_color_lpf)
}

/// Away from cancellation the estimate is the plain ratio `g·2c/(c + s)` at any level, the faint
/// ones a level-dependent `eps` would bias among them; with nothing in the low-pass values it is
/// the neighbour's green, the additive estimate there.
#[test]
fn well_conditioned_green_estimate_matches_canonical_ratio() {
    for neighbor_green in [-0.75, 0.0, 1.5] {
        for center_lpf in [1e-9, 5e-6, 1e-5, 4.0] {
            for same_color_lpf in [0.0, 1e-9, 5e-6, 8.0] {
                assert_eq!(
                    estimate(neighbor_green, center_lpf, same_color_lpf),
                    canonical_green(neighbor_green, center_lpf, same_color_lpf)
                );
            }
        }
        assert_eq!(estimate(neighbor_green, 0.0, 0.0), neighbor_green);
    }

    for (neighbor_green, center_lpf, same_color_lpf) in [(0.25, 1.0, -0.5), (-0.75, -4.0, -2.0)] {
        assert_eq!(
            estimate(neighbor_green, center_lpf, same_color_lpf),
            canonical_green(neighbor_green, center_lpf, same_color_lpf)
        );
    }
}

/// Where `c + s` cancels exactly the estimate is the additive one: `0.25 + (1 − (−1))/8`. Each
/// lane keeps its own case beside lanes in the others — the plain ratio, the blend, exact
/// cancellation, and nothing in the low-pass values — and computes what it computes alone.
#[test]
fn cancelling_green_estimate_has_the_additive_limit() {
    assert_eq!(estimate(0.25, 1.0, -1.0), 0.25 + 2.0 / 8.0);

    let cases: [[f32; 3]; F32_LANES] = [
        [0.25, 1.0, -1.0],
        [0.25, 1.0, -0.5],
        [0.5, 0.0, 0.0],
        [0.25, 1.0, -0.9],
        [-0.75, -4.0, -2.0],
        [1.5, 0.0, -0.0],
        [0.25, -1.0, 1.0],
        [0.1, 2.0, 0.5],
    ];
    let isa = Portable::new();
    let lanes = |input: usize| isa.load_f32(&cases.map(|case| case[input]));
    let mixed = estimate_green(isa, lanes(0), lanes(1), lanes(2)).to_array();
    for (case, lane) in cases.into_iter().zip(mixed) {
        let [green, center, same] = case;
        assert_eq!(
            lane.to_bits(),
            estimate(green, center, same).to_bits(),
            "{case:?}"
        );
    }
}

/// At `s = −c·(1 − ρ/2)/(1 + ρ/2)` the denominator is `c·ρ/(1 + ρ/2)` and `|c| + |s|` is
/// `2c/(1 + ρ/2)`, so `t` is a half: the smoothstep weight `t²(3 − 2t)` is a half too, and the
/// estimate the midpoint of the additive one and the ratio.
#[test]
fn cancelling_green_estimate_blends_halfway_at_half_the_condition_limit() {
    let neighbor_green = 0.25;
    let center_lpf = 1.0;
    let condition = 0.5 * MIN_SIGNED_DENOMINATOR_RATIO;
    let same_color_lpf = -center_lpf * (1.0 - condition) / (1.0 + condition);
    let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
    let canonical = canonical_green(neighbor_green, center_lpf, same_color_lpf);
    let expected = f32::midpoint(additive, canonical);
    let actual = estimate(neighbor_green, center_lpf, same_color_lpf);

    assert!((actual - expected).abs() < 1e-6);
}

/// The estimate is continuous where it switches from the ratio to the blend. At the switch,
/// `t = |denominator| / transition` is 1 and the blend puts all its weight on the ratio, so
/// denominators just outside and just inside give values that differ by the forms' slopes times
/// the step. In `t` the ratio's slope is the ratio itself (under 8·|green| by the condition limit)
/// and the blend's is the additive estimate, so a step of 2e-5 moves either by under
/// 2e-5 · (8·|green| + |additive|).
#[test]
fn the_green_estimate_is_continuous_across_the_switch() {
    let condition = f64::from(MIN_SIGNED_DENOMINATOR_RATIO);
    for (green, center) in [(0.25f64, 1.0f64), (-0.5, 2.0), (1.5, 0.3)] {
        // The negative same-colour LPF that puts the denominator at `t` times the transition:
        // c + s = t·R·(c − s), solved for s.
        let same_color_at = |t: f64| (t * condition * center - center) / (1.0 + t * condition);
        let at = |t: f64| estimate(green as f32, center as f32, same_color_at(t) as f32);
        let (outside, inside) = (at(1.0 + 1e-5), at(1.0 - 1e-5));
        let additive = green + (center - same_color_at(1.0)) * 0.125;
        let bound = 2e-5 * (8.0 * green.abs() + additive.abs());
        assert!(
            f64::from((outside - inside).abs()) < bound,
            "green {green}, center {center}: {outside} outside vs {inside} inside"
        );
    }
}

/// The interior of every scene, in every Bayer phase, is librtprocess's RCD to the bit.
///
/// `internals/reference/rcd_librtprocess.py` builds librtprocess's `rcd.cc` at a pinned commit and
/// prints each case's FNV-1a 64 digest of the output inside [`INTERPOLATED_BORDER`]: the planes in
/// order, then rows, then columns, each f32's little-endian bytes. The script changes two steps. One
/// is RCD 2.3's definition: the diagonal statistics of a red or blue site sum the squared high-pass
/// filter over the site and its two diagonal neighbours, which librtprocess reads partly from the
/// pixels beside them. The other drops the `eps` from the green ratio's denominator, which lumos
/// takes level-free; the scenes are positive, so the denominator never cancels. The 160×120 frame fits in one of librtprocess's 194-pixel tiles: its tiles
/// overlap by 9 pixels where RCD reaches 10, so the two columns at a seam differ from an untiled
/// run (by up to 2e-5 on the grating). The scenes use only correctly rounded operations, so their
/// samples are the same bits on every platform.
#[test]
fn rcd_matches_librtprocess_bit_for_bit() {
    /// A scene's sample of `channel` at `(x, y)`.
    type Scene = fn(usize, usize, usize) -> f32;
    const WIDTH: usize = 160;
    const HEIGHT: usize = 120;
    const DIGESTS: [[u64; 4]; 4] = [
        [
            0xa82fc7548fe993cd,
            0xc06614bb3200a17d,
            0x1441d1a8ac7075ed,
            0xd240e1743b3f3fbd,
        ],
        [
            0x147d2ffce8691499,
            0x61d922d96215fa61,
            0x33893da62abad5f7,
            0x908c75f897002fd7,
        ],
        [
            0x6f7b901afa20e846,
            0xa82fb56e9572dc68,
            0x4cae9efbfe6383c5,
            0x6cb3b650f30d34ab,
        ],
        [
            0xc64c4a23621fd5ab,
            0xb8b884d4819b0d49,
            0x999ebacc4497dd71,
            0xc1b07c80e88121db,
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
    let patterns = [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ];
    for ((name, scene), digests) in scenes.into_iter().zip(DIGESTS) {
        for (pattern, expected) in patterns.into_iter().zip(digests) {
            let data: Vec<f32> = (0..size.pixel_count())
                .map(|index| {
                    let (x, y) = (index % WIDTH, index / WIDTH);
                    scene(pattern.color_at(Vec2us::new(x, y)), x, y)
                })
                .collect();
            let planes = demosaic(
                &BayerImage::new(&data, size, pattern),
                &CancelToken::never(),
            )
            .unwrap();
            let mut digest = 0xcbf2_9ce4_8422_2325u64;
            for plane in &planes {
                for row in INTERPOLATED_BORDER..HEIGHT - INTERPOLATED_BORDER {
                    let interior = &plane[row * WIDTH + INTERPOLATED_BORDER
                        ..(row + 1) * WIDTH - INTERPOLATED_BORDER];
                    for byte in interior.iter().flat_map(|value| value.to_le_bytes()) {
                        digest = (digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
                    }
                }
            }
            assert_eq!(digest, expected, "{name}, {pattern:?}: {digest:#018x}");
        }
    }
}

/// A tile writes the same bits whatever its buffers held before: every cell a pixel it writes
/// reads, a stage computed for this tile. Run on a frame of one whole tile and on the cut-short
/// tile at its far corner, after buffers filled with +1000 and with −1000, in each Bayer phase.
#[test]
fn a_tile_writes_nothing_that_reads_another_tiles_leftovers() {
    let size = Size2us::new(TILE + 40, TILE + 30);
    let mut rng = TestRng::new(11);
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    for pattern in CfaPattern::ALL {
        for place in [
            TilePlace {
                top: 0,
                left: 0,
                size: Size2us::new(TILE, TILE),
            },
            TilePlace {
                top: size.height - 100,
                left: size.width - 90,
                size: Size2us::new(90, 100),
            },
        ] {
            let run = |poison: f32| {
                let mut planes = [(); 3].map(|()| vec![f32::NAN; size.pixel_count()]);
                let out = OutputPlanes::of(&mut planes);
                let mut tile = Tile::new(pattern);
                tile.poison(poison);
                // SAFETY: the planes cover the frame, and this is the only tile.
                unsafe { tile.demosaic(&BayerImage::new(&data, size, pattern), place, out) };
                planes
            };
            let (high, low) = (run(1000.0), run(-1000.0));
            for (high, low) in high.iter().zip(&low) {
                for (index, (a, b)) in high.iter().zip(low).enumerate() {
                    assert_eq!(
                        a.to_bits(),
                        b.to_bits(),
                        "{pattern:?}, {place:?} at {index}"
                    );
                }
            }
            let written = high[0].iter().filter(|value| !value.is_nan()).count();
            let side = |extent: usize| extent - 2 * INTERPOLATED_BORDER;
            assert_eq!(written, side(place.size.width) * side(place.size.height));
        }
    }
}

/// Under a white balance, the interior is the demosaic of the balanced samples brought back: each
/// native sample the input's own, and each interpolated one the balanced run's divided by its
/// colour's gain. Green's unit gain leaves its values as they are. On a frame of several tiles, in
/// each Bayer phase.
#[test]
fn a_balanced_demosaic_divides_the_balanced_one_by_the_gains() {
    let size = Size2us::new(TILE + 40, TILE + 30);
    let gains = [2.13, 1.0, 1.71];
    let mut rng = TestRng::new(5);
    let data: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    for pattern in CfaPattern::ALL {
        let colour = |index: usize| pattern.color_at(size.point_of(index));
        let balanced: Vec<f32> = data
            .iter()
            .enumerate()
            .map(|(index, &sample)| sample * gains[colour(index)])
            .collect();
        let never = CancelToken::never();
        let planes = demosaic(
            &BayerImage::new(&data, size, pattern).with_gains(gains),
            &never,
        )
        .unwrap();
        let reference = demosaic(&BayerImage::new(&balanced, size, pattern), &never).unwrap();
        let border = INTERPOLATED_BORDER;
        for row in border..size.height - border {
            for index in row * size.width + border..(row + 1) * size.width - border {
                for (channel, (plane, reference)) in planes.iter().zip(&reference).enumerate() {
                    let expected = if channel == colour(index) {
                        data[index]
                    } else {
                        reference[index] / gains[channel]
                    };
                    assert_eq!(
                        plane[index].to_bits(),
                        expected.to_bits(),
                        "{pattern:?} channel {channel} at {index}"
                    );
                }
            }
        }
    }
}
