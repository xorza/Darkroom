use crate::internals::prelude::*;
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::io::raw::demosaic::xtrans::internals::{
    make_xtrans, test_pattern, test_pattern_array, to_u16,
};
use crate::io::raw::demosaic::xtrans::markesteijn::*;
use crate::io::raw::demosaic::xtrans::markesteijn_steps::MARK_INFO_BORDER;

#[derive(Clone, Copy, Debug)]
enum SyntheticScene {
    ColorEdge,
    Impulse,
    Star,
    ColorGrating,
}

#[derive(Debug)]
struct GoldenSample {
    pos: Vec2us,
    rgb: [f32; 3],
}

#[derive(Debug)]
struct GoldenCase {
    scene: SyntheticScene,
    samples: [GoldenSample; 4],
}

/// The regions tile the arena in the documented order — A 4P, E 8P, B 4P, C P, D P words — so
/// each step's scratch is exactly the region its doc names, and the last ends where the arena does.
#[test]
fn arena_regions_tile_the_arena_in_order() {
    let width = 5;
    let height = 3;
    let pixels = width * height;
    let bytes_per_word = size_of::<f32>();
    let mut arena = DemosaicArena::new(Size2us::new(width, height));
    let arena_start = arena.storage.as_ptr() as usize;
    let arena_end = arena_start + arena.storage.len() * bytes_per_word;

    let regions = arena.regions();
    let mut offset = 0;
    for (name, region, words) in [
        ("A", &*regions.a, 4),
        ("E", &*regions.e, 8),
        ("B", &*regions.b, 4),
        ("C", &*regions.c, 1),
        ("D", &*regions.d, 1),
    ] {
        assert_eq!(region.len(), words * pixels, "{name}");
        assert_eq!(
            region.as_ptr() as usize,
            arena_start + offset * bytes_per_word,
            "{name}"
        );
        offset += words * pixels;
    }
    assert_eq!(arena_start + offset * bytes_per_word, arena_end);
}

fn synthetic_value(scene: SyntheticScene, channel: usize, pos: Vec2us) -> f32 {
    const WIDTH: usize = 96;
    const HEIGHT: usize = 96;

    match scene {
        SyntheticScene::ColorEdge => {
            let left = [0.1, 0.3, 0.8];
            let right = [0.9, 0.6, 0.2];
            if pos.x < WIDTH / 2 {
                left[channel]
            } else {
                right[channel]
            }
        }
        SyntheticScene::Impulse => {
            if pos.x == WIDTH / 2 && pos.y == HEIGHT / 2 {
                [1.0, 0.7, 0.4][channel]
            } else {
                0.05
            }
        }
        SyntheticScene::Star => {
            let dx = pos.x as f32 - (WIDTH - 1) as f32 * 0.5;
            let dy = pos.y as f32 - (HEIGHT - 1) as f32 * 0.5;
            let sigma = [1.2_f32, 1.6, 2.0][channel];
            let amplitude = [0.9_f32, 0.7, 0.5][channel];
            0.02 + amplitude * (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp()
        }
        SyntheticScene::ColorGrating => {
            let phase = [0.0_f32, 2.094_395_2, 4.188_790_3][channel];
            0.5 + 0.4 * (0.47 * pos.x as f32 + 0.31 * pos.y as f32 + phase).sin()
        }
    }
}

#[test]
#[expect(
    clippy::excessive_precision,
    reason = "the reference values are pasted as librtprocess printed them"
)]
fn markesteijn_matches_librtprocess_reference_scenes() {
    const WIDTH: usize = 96;
    const HEIGHT: usize = 96;
    const TOLERANCE: f32 = 5e-6;
    // These scalar golden values avoid librtprocess's SSE YPbPr coefficient-order bug.
    let cases = [
        GoldenCase {
            scene: SyntheticScene::ColorEdge,
            samples: [
                GoldenSample {
                    pos: Vec2us::new(47, 48),
                    rgb: [0.099_999_994, 0.300_000_012, 0.800_000_012],
                },
                GoldenSample {
                    pos: Vec2us::new(48, 48),
                    rgb: [0.899_999_976, 0.600_000_024, 0.199_999_988],
                },
                GoldenSample {
                    pos: Vec2us::new(49, 48),
                    rgb: [0.899_999_976, 0.600_000_024, 0.200_000_018],
                },
                GoldenSample {
                    pos: Vec2us::new(50, 48),
                    rgb: [0.899_999_976, 0.600_000_024, 0.199_999_988],
                },
            ],
        },
        GoldenCase {
            scene: SyntheticScene::Impulse,
            samples: [
                GoldenSample {
                    pos: Vec2us::new(48, 48),
                    rgb: [0.552_734_375, 0.699_999_988, 0.552_734_375],
                },
                GoldenSample {
                    pos: Vec2us::new(48, 47),
                    rgb: [0.050_000_000_7, 0.270_898_432, 0.270_898_432],
                },
                GoldenSample {
                    pos: Vec2us::new(47, 48),
                    rgb: [0.270_898_432, 0.270_898_432, 0.050_000_000_7],
                },
                GoldenSample {
                    pos: Vec2us::new(49, 49),
                    rgb: [0.050_000_004_5, 0.050_000_000_7, 0.050_000_004_5],
                },
            ],
        },
        GoldenCase {
            scene: SyntheticScene::Star,
            samples: [
                GoldenSample {
                    pos: Vec2us::new(47, 47),
                    rgb: [0.653_244_376, 0.654_872_417, 0.588_915_467],
                },
                GoldenSample {
                    pos: Vec2us::new(50, 47),
                    rgb: [0.110_830_717, 0.216_674_328, 0.257_652_014],
                },
                GoldenSample {
                    pos: Vec2us::new(47, 52),
                    rgb: [0.029_506_173, 0.032_290_011_6, 0.058_555_860_1],
                },
                GoldenSample {
                    pos: Vec2us::new(48, 48),
                    rgb: [0.673_444_748, 0.654_872_417, 0.579_427_004],
                },
            ],
        },
        GoldenCase {
            scene: SyntheticScene::ColorGrating,
            samples: [
                GoldenSample {
                    pos: Vec2us::new(31, 24),
                    rgb: [0.405_019_253, 0.157_421_41, 0.712_104_738],
                },
                GoldenSample {
                    pos: Vec2us::new(48, 48),
                    rgb: [0.447_177_649, 0.886_090_875, 0.324_239_552],
                },
                GoldenSample {
                    pos: Vec2us::new(65, 70),
                    rgb: [0.694_080_234, 0.141_468_421, 0.456_137_031],
                },
                GoldenSample {
                    pos: Vec2us::new(63, 32),
                    rgb: [0.886_546_731, 0.217_050_105, 0.379_481_941],
                },
            ],
        },
    ];
    let pattern = test_pattern_array();

    for case in cases {
        let mut data = vec![0.0; WIDTH * HEIGHT];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let channel = pattern[y % 6][x % 6] as usize;
                data[y * WIDTH + x] = synthetic_value(case.scene, channel, Vec2us::new(x, y));
            }
        }
        let size = Size2us::new(WIDTH, HEIGHT);
        let xtrans =
            XTransImage::with_margins_f32(&data, SensorLayout::cropped(size), test_pattern());
        let planes = demosaic(&xtrans, &CancelToken::never()).unwrap();
        for sample in case.samples {
            let index = size.index_of(sample.pos);
            for (channel, plane) in planes.iter().enumerate() {
                let actual = plane[index];
                let expected = sample.rgb[channel];
                assert!(
                    (actual - expected).abs() <= TOLERANCE,
                    "{:?} ({}, {}) channel {}: {actual} != {expected}",
                    case.scene,
                    sample.pos.x,
                    sample.pos.y,
                    channel,
                );
            }
        }
    }
}

/// A constant colour per channel — uniform grey and two distinct (R, G, B) — comes back as itself
/// at every pixel, border included, with and without masked margins. Every stage averages or
/// blends equal values, so each output is the input to a few f32 roundings: under 2e-7, where one
/// rounding of a value below 1 is up to 6e-8.
#[test]
fn constant_colour_reconstructs_to_rounding() {
    let active = Size2us::new(36, 36);
    let pattern = test_pattern();
    for colour in [[0.5f32; 3], [0.8, 0.5, 0.2], [0.1, 0.9, 0.4]] {
        // Margins of 6 keep the 6×6 layout's phase at the raw origin.
        for margin in [0, 6] {
            let raw = Size2us::new(active.width + 2 * margin, active.height + 2 * margin);
            let data: Vec<f32> = (0..raw.pixel_count())
                .map(|index| colour[pattern.color_at(raw.point_of(index)) as usize])
                .collect();
            let layout = SensorLayout {
                raw,
                active,
                margin: Vec2us::new(margin, margin),
            };
            let planes = demosaic(
                &XTransImage::with_margins_f32(&data, layout, pattern),
                &CancelToken::never(),
            )
            .unwrap();
            for (channel, plane) in planes.iter().enumerate() {
                for (index, &value) in plane.iter().enumerate() {
                    assert_close!(
                        value,
                        colour[channel],
                        2e-7,
                        "{colour:?} margin {margin} channel {channel} at {index}: {value}"
                    );
                }
            }
        }
    }
}

#[test]
fn markesteijn_no_nan() {
    let raw_w = 30;
    let raw_h = 30;
    let w = 18;
    let h = 18;
    let data: Vec<u16> = (0..raw_w * raw_h)
        .map(|i| to_u16(i as f32 / (raw_w * raw_h) as f32))
        .collect();
    let xtrans = make_xtrans(
        &data,
        SensorLayout {
            raw: Size2us::new(raw_w, raw_h),
            active: Size2us::new(w, h),
            margin: Vec2us::new(6, 6),
        },
    );

    let planes = demosaic(&xtrans, &CancelToken::never()).unwrap();

    for (i, &v) in planes.iter().flatten().enumerate() {
        assert!(v.is_finite(), "NaN/Inf at pixel {i}");
    }
}

#[test]
fn markesteijn_all_zeros() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let data = vec![0u16; raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        SensorLayout {
            raw: Size2us::new(raw_w, raw_h),
            active: Size2us::new(w, h),
            margin: Vec2us::new(6, 6),
        },
    );

    let planes = demosaic(&xtrans, &CancelToken::never()).unwrap();
    for &v in planes.iter().flatten() {
        assert_eq!(v, 0.0, "Expected 0.0 for all-zero input");
    }
}

/// From the border fill in, a frame demosaics bit for bit as the same pixels inside a larger
/// frame: no stage reads a value it did not compute from the frame's own samples. Random samples,
/// so no stencil can hide behind equal neighbours; an offset of 12 keeps the 6×6 layout's phase.
#[test]
fn markesteijn_beyond_the_border_matches_a_larger_frame() {
    let large = Size2us::new(96, 96);
    let offset = 12;
    let mut rng = TestRng::new(7);
    let samples: Vec<f32> = (0..large.pixel_count())
        .map(|_| 0.1 + 0.8 * rng.next_f32())
        .collect();
    let small = Size2us::new(60, 60);
    let crop: Vec<f32> = (0..small.pixel_count())
        .map(|index| {
            let pos = small.point_of(index);
            samples[large.index_of(Vec2us::new(pos.x + offset, pos.y + offset))]
        })
        .collect();
    let run = |data: &[f32], size| {
        let xtrans =
            XTransImage::with_margins_f32(data, SensorLayout::cropped(size), test_pattern());
        demosaic(&xtrans, &CancelToken::never()).unwrap()
    };
    let whole = run(&samples, large);
    let part = run(&crop, small);
    for (channel, (part_plane, whole_plane)) in part.iter().zip(&whole).enumerate() {
        for (index, value) in part_plane.iter().enumerate() {
            let pos = small.point_of(index);
            let distance = pos
                .x
                .min(pos.y)
                .min(small.width - 1 - pos.x)
                .min(small.height - 1 - pos.y);
            if distance < MARK_INFO_BORDER {
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
