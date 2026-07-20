//! Markesteijn 1-pass demosaicing for X-Trans sensors.
//!
//! Implements Frank Markesteijn's directional interpolation algorithm with
//! homogeneity-based direction selection. Produces significantly better quality
//! than bilinear interpolation, especially for star profiles in astrophotography.
//!
//! The algorithm:
//! 1. Interpolates green in 4 directions using weighted hexagonal neighbors
//! 2. Reconstructs red and blue with Markesteijn's three geometry-specific stages
//! 3. Computes perceptual derivatives from the directional RGB candidates
//! 4. Builds homogeneity maps to identify the best direction(s) per pixel
//! 5. Blends the best directions into the final RGB output
//!
//! Performance: targets <500ms for 6032×4028 (vs libraw's 1750ms single-threaded).
//!
//! ## Memory layout
//!
//! All working memory is preallocated in a single contiguous arena (`DemosaicArena`)
//! so the peak is explicit and visible. Buffers with non-overlapping lifetimes share
//! the same memory region:
//!
//! ```text
//! [ A: green_dir (4P) | E: red_blue_dir (8P) | B: drv (4P) | C: gmin/homo (P) | D: gmax/threshold (P) ]
//! Total: 18P f32 arena, where P = width × height (+ 3P for the planar output buffers)
//! ```
//!
//! Region A holds green_dir (4 directions), written in Step 2, read through Step 6.
//! Region E holds directional `[red, blue]` pairs, written in Step 3 and read through Step 6.
//! Region B is used as `drv` in Steps 4–5, then dead in Step 6.
//! Region C is used as `gmin` in Steps 1–2, then reinterpreted as `homo` (u8) in Steps 5–6.
//! Region D is used as `gmax` in Steps 1–2, then reused as `threshold` in Step 5.

use common::CancelToken;

use crate::io::raw::alloc_uninit_vec;
use crate::io::raw::demosaic::Cancelled;
use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::markesteijn_steps;

/// Number of interpolation directions (4 for 1-pass: H, V, D1, D2).
pub(crate) const NDIR: usize = 4;

/// Preallocated arena for all Markesteijn demosaic working memory.
///
/// Single contiguous allocation with regions that are reused across steps.
/// See module-level docs for the full layout and lifetime diagram.
#[derive(Debug)]
struct DemosaicArena {
    storage: Vec<f32>,
}

impl DemosaicArena {
    fn new(width: usize, height: usize) -> Self {
        let pixels = width * height;
        let total = 18 * pixels; // 4P + 8P + 4P + P + P

        // SAFETY: Every element in every region is fully written by parallel passes
        // before being read. See per-step comments in demosaic_xtrans_markesteijn().
        let storage = unsafe { alloc_uninit_vec::<f32>(total) };

        tracing::debug!(
            "Demosaic arena: {:.1} MB ({} × {} × 18 × 4 bytes)",
            (total * 4) as f64 / (1024.0 * 1024.0),
            width,
            height,
        );

        Self { storage }
    }
}

/// Demosaic an X-Trans image using Markesteijn 1-pass algorithm.
///
/// Returns unclipped planar channels `[R, G, B]`, each `width * height`.
pub(crate) fn demosaic_xtrans_markesteijn(
    xtrans: &XTransImage,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    use std::time::Instant;

    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;

    // Build lookup tables
    let hex = HexLookup::new(&xtrans.raw_pattern);
    // Allocate all working memory in one shot
    let mut arena = DemosaicArena::new(width, height);

    // Step 1: Compute green min/max bounds for non-green pixels
    // Writes: Region C (gmin), Region D (gmax)
    let t = Instant::now();
    {
        let (before_d, region_d) = arena.storage.split_at_mut(17 * pixels);
        let region_c = &mut before_d[16 * pixels..];
        markesteijn_steps::compute_green_minmax(xtrans, &hex, region_c, region_d);
    }
    tracing::debug!(
        "  Step 1 (green min/max): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 2: Interpolate green in 4 directions
    // Reads: Region C (gmin), Region D (gmax). Writes: Region A (green_dir).
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let t = Instant::now();
    {
        let (before_c, cd) = arena.storage.split_at_mut(16 * pixels);
        let region_a = &mut before_c[..4 * pixels];
        let (region_c, region_d) = cd.split_at(pixels);
        markesteijn_steps::interpolate_green(xtrans, &hex, region_c, region_d, region_a);
    }
    tracing::debug!(
        "  Step 2 (green interp): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 3: Reconstruct red and blue using the three canonical geometry stages.
    // Reads: Region A (green_dir). Writes: Region E (red_blue_dir).
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let t = Instant::now();
    {
        let (region_a, rest) = arena.storage.split_at_mut(4 * pixels);
        let region_e = &mut rest[..8 * pixels];
        // SAFETY: `[f32; 2]` has the same alignment as f32 and exactly covers Region E.
        let colors = unsafe {
            std::slice::from_raw_parts_mut(region_e.as_mut_ptr() as *mut [f32; 2], NDIR * pixels)
        };
        markesteijn_steps::reconstruct_colors(xtrans, &hex, region_a, colors);
    }
    tracing::debug!(
        "  Step 3 (red/blue reconstruction): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 4: Compute YPbPr derivatives.
    // Reads: Regions A and E. Writes: Region B.
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let t = Instant::now();
    {
        let (region_a, rest) = arena.storage.split_at_mut(4 * pixels);
        let (region_e, rest) = rest.split_at_mut(8 * pixels);
        let region_b = &mut rest[..4 * pixels];
        // SAFETY: Region E was fully initialized as `[f32; 2]` in Step 3.
        let colors = unsafe {
            std::slice::from_raw_parts(region_e.as_ptr() as *const [f32; 2], NDIR * pixels)
        };
        markesteijn_steps::compute_derivatives(xtrans, region_a, colors, region_b);
    }
    tracing::debug!(
        "  Step 4 (derivatives): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 5: Build homogeneity maps from derivatives.
    // Reads: Region B. Writes: Region C (homo via u8 reinterpret), Region D (threshold).
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let t = Instant::now();
    {
        let (before_d, region_d) = arena.storage.split_at_mut(17 * pixels);
        let (before_c, region_c) = before_d.split_at_mut(16 * pixels);
        let drv = &before_c[12 * pixels..];
        // SAFETY: Region C (f32 at [16P..17P]) reinterpreted as u8 for homo.
        // gmin data is dead after Step 2. f32 alignment (4) satisfies u8 alignment (1).
        let homo =
            unsafe { std::slice::from_raw_parts_mut(region_c.as_mut_ptr() as *mut u8, pixels * 4) };
        markesteijn_steps::compute_homogeneity(drv, width, height, homo, region_d);
    }
    tracing::debug!(
        "  Step 5 (homogeneity): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 6: Final blend.
    // Reads: Regions A, E, and C (homo via SATs). Writes planar [R, G, B]
    // directly into the output buffers — no interleave, no extract copy.
    // SAFETY: blend_final writes every element of each output buffer.
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let mut r = unsafe { alloc_uninit_vec::<f32>(pixels) };
    let mut g = unsafe { alloc_uninit_vec::<f32>(pixels) };
    let mut b = unsafe { alloc_uninit_vec::<f32>(pixels) };
    let t = Instant::now();
    {
        let green_dir = &arena.storage[..4 * pixels];
        // SAFETY: Region E was fully initialized as `[f32; 2]` in Step 3.
        let colors = unsafe {
            let ptr = arena.storage[4 * pixels..].as_ptr() as *const [f32; 2];
            std::slice::from_raw_parts(ptr, NDIR * pixels)
        };
        // SAFETY: Region C (f32 at [16P..17P]) holds the homogeneity map written as u8
        // in Step 5; f32 alignment (4) satisfies u8 alignment (1). Read-only here.
        let homo = unsafe {
            let ptr = arena.storage[16 * pixels..].as_ptr() as *const u8;
            std::slice::from_raw_parts(ptr, pixels * 4)
        };
        markesteijn_steps::blend_final(
            xtrans, green_dir, colors, homo, width, height, &mut r, &mut g, &mut b,
        );
    }
    tracing::debug!(
        "  Step 6 (blend): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    Ok([r, g, b])
}

#[cfg(test)]
mod tests {
    use crate::io::raw::demosaic::interleave_planes;
    use crate::io::raw::demosaic::xtrans::markesteijn::*;
    use crate::io::raw::demosaic::xtrans::test_support::{
        TEST_INV_RANGE, make_xtrans, test_pattern, test_pattern_array, to_u16,
    };

    #[derive(Clone, Copy, Debug)]
    enum SyntheticScene {
        ColorEdge,
        Impulse,
        Star,
        ColorGrating,
    }

    #[derive(Debug)]
    struct GoldenSample {
        x: usize,
        y: usize,
        rgb: [f32; 3],
    }

    #[derive(Debug)]
    struct GoldenCase {
        scene: SyntheticScene,
        samples: [GoldenSample; 4],
    }

    fn synthetic_value(scene: SyntheticScene, channel: usize, x: usize, y: usize) -> f32 {
        const WIDTH: usize = 96;
        const HEIGHT: usize = 96;

        match scene {
            SyntheticScene::ColorEdge => {
                let left = [0.1, 0.3, 0.8];
                let right = [0.9, 0.6, 0.2];
                if x < WIDTH / 2 {
                    left[channel]
                } else {
                    right[channel]
                }
            }
            SyntheticScene::Impulse => {
                if x == WIDTH / 2 && y == HEIGHT / 2 {
                    [1.0, 0.7, 0.4][channel]
                } else {
                    0.05
                }
            }
            SyntheticScene::Star => {
                let dx = x as f32 - (WIDTH - 1) as f32 * 0.5;
                let dy = y as f32 - (HEIGHT - 1) as f32 * 0.5;
                let sigma = [1.2_f32, 1.6, 2.0][channel];
                let amplitude = [0.9_f32, 0.7, 0.5][channel];
                0.02 + amplitude * (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp()
            }
            SyntheticScene::ColorGrating => {
                let phase = [0.0_f32, 2.094_395_2, 4.188_790_3][channel];
                0.5 + 0.4 * (0.47 * x as f32 + 0.31 * y as f32 + phase).sin()
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
                        x: 47,
                        y: 48,
                        rgb: [0.099_999_994, 0.300_000_012, 0.800_000_012],
                    },
                    GoldenSample {
                        x: 48,
                        y: 48,
                        rgb: [0.899_999_976, 0.600_000_024, 0.199_999_988],
                    },
                    GoldenSample {
                        x: 49,
                        y: 48,
                        rgb: [0.899_999_976, 0.600_000_024, 0.200_000_018],
                    },
                    GoldenSample {
                        x: 50,
                        y: 48,
                        rgb: [0.899_999_976, 0.600_000_024, 0.199_999_988],
                    },
                ],
            },
            GoldenCase {
                scene: SyntheticScene::Impulse,
                samples: [
                    GoldenSample {
                        x: 48,
                        y: 48,
                        rgb: [0.552_734_375, 0.699_999_988, 0.552_734_375],
                    },
                    GoldenSample {
                        x: 48,
                        y: 47,
                        rgb: [0.050_000_000_7, 0.270_898_432, 0.270_898_432],
                    },
                    GoldenSample {
                        x: 47,
                        y: 48,
                        rgb: [0.270_898_432, 0.270_898_432, 0.050_000_000_7],
                    },
                    GoldenSample {
                        x: 49,
                        y: 49,
                        rgb: [0.050_000_004_5, 0.050_000_000_7, 0.050_000_004_5],
                    },
                ],
            },
            GoldenCase {
                scene: SyntheticScene::Star,
                samples: [
                    GoldenSample {
                        x: 47,
                        y: 47,
                        rgb: [0.653_244_376, 0.654_872_417, 0.588_915_467],
                    },
                    GoldenSample {
                        x: 50,
                        y: 47,
                        rgb: [0.110_830_717, 0.216_674_328, 0.257_652_014],
                    },
                    GoldenSample {
                        x: 47,
                        y: 52,
                        rgb: [0.029_506_173, 0.032_290_011_6, 0.058_555_860_1],
                    },
                    GoldenSample {
                        x: 48,
                        y: 48,
                        rgb: [0.673_444_748, 0.654_872_417, 0.579_427_004],
                    },
                ],
            },
            GoldenCase {
                scene: SyntheticScene::ColorGrating,
                samples: [
                    GoldenSample {
                        x: 31,
                        y: 24,
                        rgb: [0.405_019_253, 0.157_421_41, 0.712_104_738],
                    },
                    GoldenSample {
                        x: 48,
                        y: 48,
                        rgb: [0.447_177_649, 0.886_090_875, 0.324_239_552],
                    },
                    GoldenSample {
                        x: 65,
                        y: 70,
                        rgb: [0.694_080_234, 0.141_468_421, 0.456_137_031],
                    },
                    GoldenSample {
                        x: 63,
                        y: 32,
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
                    data[y * WIDTH + x] = synthetic_value(case.scene, channel, x, y);
                }
            }
            let xtrans = XTransImage::with_margins_f32(
                &data,
                WIDTH,
                HEIGHT,
                WIDTH,
                HEIGHT,
                0,
                0,
                test_pattern(),
            );
            let planes = demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never()).unwrap();
            for sample in case.samples {
                let index = sample.y * WIDTH + sample.x;
                for (channel, plane) in planes.iter().enumerate() {
                    let actual = plane[index];
                    let expected = sample.rgb[channel];
                    assert!(
                        (actual - expected).abs() <= TOLERANCE,
                        "{:?} ({}, {}) channel {}: {actual} != {expected}",
                        case.scene,
                        sample.x,
                        sample.y,
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
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb =
            interleave_planes(demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never()).unwrap());
        assert_eq!(rgb.len(), w * h * 3);
    }

    #[test]
    fn test_markesteijn_uniform_input() {
        let raw_w = 30;
        let raw_h = 30;
        let w = 18;
        let h = 18;
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb =
            interleave_planes(demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never()).unwrap());

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
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb =
            interleave_planes(demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never()).unwrap());

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
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb =
            interleave_planes(demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never()).unwrap());
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
            raw_w,
            raw_h,
            w,
            h,
            top,
            left,
            pattern.clone(),
            [0.0; 3],
            TEST_INV_RANGE,
            None,
        );

        let rgb =
            interleave_planes(demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never()).unwrap());

        // At green pixel positions, the green channel should be approximately the raw value
        for y in 0..h {
            for x in 0..w {
                let raw_y = y + top;
                let raw_x = x + left;
                if pattern.color_at(raw_y, raw_x) == 1 {
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
}
