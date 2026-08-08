use crate::io::raw::demosaic::interleave_planes;
use crate::io::raw::demosaic::xtrans::internals::{
    TEST_INV_RANGE, make_xtrans, test_pattern, test_pattern_array, to_u16,
};
use crate::io::raw::demosaic::xtrans::markesteijn::*;
use crate::math::vec2us::Vec2us;

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

#[test]
fn final_blend_scratch_reuses_the_exact_dead_arena_regions() {
    let width = 5;
    let height = 3;
    let pixels = width * height;
    let bytes_per_word = std::mem::size_of::<f32>();
    let mut arena = DemosaicArena::new(Size2us::new(width, height));
    arena.storage.fill(0.0);
    let arena_start = arena.storage.as_ptr() as usize;
    let arena_end = arena_start + arena.storage.len() * bytes_per_word;

    let buffers = arena.final_blend_buffers();

    assert_eq!(buffers.green_dir.len(), 4 * pixels);
    assert_eq!(buffers.colors.len(), 4 * pixels);
    assert_eq!(buffers.scores.len(), pixels);
    assert_eq!(buffers.homo.len(), 4 * pixels);
    assert_eq!(buffers.sat.len(), pixels);
    assert_eq!(buffers.green_dir.as_ptr() as usize, arena_start);
    assert_eq!(
        buffers.colors.as_ptr().cast::<f32>() as usize,
        arena_start + 4 * pixels * bytes_per_word
    );
    assert_eq!(
        buffers.scores.as_ptr().cast::<u32>() as usize,
        arena_start + 12 * pixels * bytes_per_word
    );
    assert_eq!(
        buffers.homo.as_ptr() as usize,
        arena_start + 16 * pixels * bytes_per_word
    );
    assert_eq!(
        buffers.sat.as_ptr() as usize,
        arena_start + 17 * pixels * bytes_per_word
    );
    assert_eq!(
        buffers.sat.as_ptr().wrapping_add(pixels) as usize,
        arena_end
    );
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
#[allow(clippy::excessive_precision)]
fn test_markesteijn_matches_librtprocess_reference_scenes() {
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
        let xtrans = XTransImage::with_margins_f32(&data, size, size, Vec2us::ZERO, test_pattern());
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

#[test]
fn test_markesteijn_output_size() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );

    let rgb = interleave_planes(demosaic(&xtrans, &CancelToken::never()).unwrap());
    assert_eq!(rgb.len(), w * h * 3);
}

#[test]
fn test_markesteijn_uniform_input() {
    let raw_w = 30;
    let raw_h = 30;
    let w = 18;
    let h = 18;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );

    let rgb = interleave_planes(demosaic(&xtrans, &CancelToken::never()).unwrap());

    // Uniform input should produce approximately uniform output
    for (i, &v) in rgb.iter().enumerate() {
        assert!(
            (v - 0.5).abs() < 0.05,
            "Pixel {} = {} (expected ~0.5)",
            i,
            v
        );
    }
}

#[test]
fn test_markesteijn_no_nan() {
    let raw_w = 30;
    let raw_h = 30;
    let w = 18;
    let h = 18;
    let data: Vec<u16> = (0..raw_w * raw_h)
        .map(|i| to_u16(i as f32 / (raw_w * raw_h) as f32))
        .collect();
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );

    let rgb = interleave_planes(demosaic(&xtrans, &CancelToken::never()).unwrap());

    for (i, &v) in rgb.iter().enumerate() {
        assert!(v.is_finite(), "NaN/Inf at pixel {}", i);
    }
}

#[test]
fn test_markesteijn_all_zeros() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let data = vec![0u16; raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );

    let rgb = interleave_planes(demosaic(&xtrans, &CancelToken::never()).unwrap());
    for &v in &rgb {
        assert_eq!(v, 0.0, "Expected 0.0 for all-zero input");
    }
}

#[test]
fn test_markesteijn_preserves_green_at_green_pixel() {
    let raw_w = 30;
    let raw_h = 30;
    let w = 18;
    let h = 18;
    let top = 6;
    let left = 6;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let pattern = test_pattern();
    let xtrans = XTransImage::with_margins(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(left, top),
        pattern.clone(),
        [0.0; 3],
        TEST_INV_RANGE,
        None,
    );

    let rgb = interleave_planes(demosaic(&xtrans, &CancelToken::never()).unwrap());

    // At green pixel positions, the green channel should be approximately the raw value
    for y in 0..h {
        for x in 0..w {
            let raw_y = y + top;
            let raw_x = x + left;
            if pattern.color_at(Vec2us::new(raw_x, raw_y)) == 1 {
                let g = rgb[(y * w + x) * 3 + 1];
                assert!(
                    (g - 0.5).abs() < 0.001,
                    "Green at ({},{}) = {} (expected ~0.5)",
                    y,
                    x,
                    g
                );
            }
        }
    }
}
