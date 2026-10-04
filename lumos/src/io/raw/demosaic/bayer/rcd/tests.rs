use common::CancelToken;

use crate::io::raw::demosaic::bayer::rcd::{
    EPS, INTERPOLATED_BORDER, MIN_SIGNED_DENOMINATOR_RATIO, demosaic, estimate_green,
};
use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

fn canonical_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    neighbor_green * (center_lpf + center_lpf) / (EPS + center_lpf + same_color_lpf)
}

#[test]
fn well_conditioned_green_estimate_matches_canonical_ratio() {
    for neighbor_green in [-0.75, 0.0, 1.5] {
        for center_lpf in [0.0, EPS * 0.5, EPS, 4.0] {
            for same_color_lpf in [0.0, EPS * 0.5, EPS, 8.0] {
                assert_eq!(
                    estimate_green(neighbor_green, center_lpf, same_color_lpf),
                    canonical_green(neighbor_green, center_lpf, same_color_lpf)
                );
            }
        }
    }

    for (neighbor_green, center_lpf, same_color_lpf) in [(0.25, 1.0, -0.5), (-0.75, -4.0, -2.0)] {
        assert_eq!(
            estimate_green(neighbor_green, center_lpf, same_color_lpf),
            canonical_green(neighbor_green, center_lpf, same_color_lpf)
        );
    }
}

#[test]
fn cancelling_green_estimate_has_the_additive_limit() {
    let center_lpf = 1.0;
    let same_color_lpf = -(EPS + center_lpf);
    let actual = estimate_green(0.25, center_lpf, same_color_lpf);
    let expected = 0.25 + (1.0 - same_color_lpf) / 8.0;
    assert_eq!(actual, expected);
}

#[test]
fn cancelling_green_estimate_blends_halfway_at_half_the_condition_limit() {
    let neighbor_green = 0.25;
    let center_lpf = 1.0;
    let condition = 0.5 * MIN_SIGNED_DENOMINATOR_RATIO;
    let same_color_lpf = -(EPS + center_lpf) * (1.0 - condition) / (1.0 + condition);
    let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
    let canonical = canonical_green(neighbor_green, center_lpf, same_color_lpf);
    let expected = f32::midpoint(additive, canonical);
    let actual = estimate_green(neighbor_green, center_lpf, same_color_lpf);

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
    let epsilon = f64::from(EPS);
    for (green, center) in [(0.25f64, 1.0f64), (-0.5, 2.0), (1.5, 0.3)] {
        // The negative same-colour LPF that puts the denominator at `t` times the transition:
        // EPS + c + s = t·R·(EPS + c − s), solved for s.
        let same_color_at = |t: f64| {
            (t * condition * (epsilon + center) - (epsilon + center)) / (1.0 + t * condition)
        };
        let at = |t: f64| estimate_green(green as f32, center as f32, same_color_at(t) as f32);
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
/// order, then rows, then columns, each f32's little-endian bytes. The script changes one step, to
/// RCD 2.3's definition: the diagonal statistics of a red or blue site sum the squared high-pass
/// filter over the site and its two diagonal neighbours, which librtprocess reads partly from the
/// pixels beside them. The 160×120 frame fits in one of librtprocess's 194-pixel tiles: its tiles
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
            0xf14078aaa3c6e9e5,
            0x4f104cd33116b4dd,
            0xf3cbc1ac0c453ebd,
            0x5704fd0e5d293435,
        ],
        [
            0xe8cf0df219edb161,
            0x69ecfc47410bf262,
            0x57bb4d1c6d876e6b,
            0x9aefe44a56f558e3,
        ],
        [
            0x7e0441325eff5d06,
            0x541d9bc3195adc63,
            0x46db350ff0d57f84,
            0x2d8a4d4c3aed8b95,
        ],
        [
            0x393e7ca60ee00a24,
            0x6189158b963eece6,
            0xce5f6f859635119f,
            0x9b961d4d52b177af,
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
