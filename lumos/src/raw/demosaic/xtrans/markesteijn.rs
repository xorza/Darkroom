//! Markesteijn 1-pass demosaicing for X-Trans sensors.
//!
//! Implements Frank Markesteijn's directional interpolation algorithm with
//! homogeneity-based direction selection. Produces significantly better quality
//! than bilinear interpolation, especially for star profiles in astrophotography.
//!
//! The algorithm:
//! 1. Interpolates green in 4 directions using weighted hexagonal neighbors
//! 2. Interpolates R/B using green as a guide signal
//! 3. Computes perceptual derivatives per direction
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
//! [ ---- Region A: rgb_dir (12P) ---- | Region B: green_dir/drv (4P) | C: gmin/homo (P) | D: gmax/threshold (P) ]
//! Total: 18P f32 elements, where P = width × height
//! ```
//!
//! Region B is used as `green_dir` in Steps 1–3, then overwritten as `drv` in Steps 3–6.
//! Region C is used as `gmin` in Steps 1–2, then reinterpreted as `homo` (u8) in Steps 5–6.
//! Region D is used as `gmax` in Steps 1–2, then reused as `threshold` in Step 5.

use super::XTransImage;
use super::hex_lookup::HexLookup;
use super::markesteijn_steps;

/// Number of interpolation directions (4 for 1-pass: H, V, D1, D2).
pub(super) const NDIR: usize = 4;

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
        let total = 18 * pixels; // 12P + 4P + P + P

        // SAFETY: Every element in every region is fully written by parallel passes
        // before being read. See per-step comments in demosaic_xtrans_markesteijn().
        let storage = unsafe { crate::raw::alloc_uninit_vec::<f32>(total) };

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
/// Returns RGB interleaved pixels: [R0, G0, B0, R1, G1, B1, ...] in 0.0-1.0 range.
pub fn demosaic_xtrans_markesteijn(xtrans: &XTransImage) -> Vec<f32> {
    use std::time::Instant;

    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;

    // Build hex neighbor lookup tables
    let hex = HexLookup::new(&xtrans.pattern);

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
    // Reads: Region C (gmin), Region D (gmax). Writes: Region B (green_dir).
    let t = Instant::now();
    {
        let (ab, cd) = arena.storage.split_at_mut(16 * pixels);
        let region_b = &mut ab[12 * pixels..16 * pixels];
        let (region_c, region_d) = cd.split_at(pixels);
        markesteijn_steps::interpolate_green(xtrans, &hex, region_c, region_d, region_b);
    }
    tracing::debug!(
        "  Step 2 (green interp): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 3: Compute per-direction RGB from green_dir
    // Reads: Region B (as green_dir). Writes: Region A (rgb_dir).
    // After this pass, Region B's green_dir data is dead.
    let t = Instant::now();
    {
        let (region_a, rest) = arena.storage.split_at_mut(12 * pixels);
        let region_b = &rest[..4 * pixels];
        markesteijn_steps::compute_rgb(xtrans, region_b, region_a);
    }

    // Step 4: Compute YPbPr derivatives from rgb_dir
    // Reads: Region A (rgb_dir). Writes: Region B (now drv, overwriting dead green_dir).
    {
        let (region_a, rest) = arena.storage.split_at_mut(12 * pixels);
        let region_b = &mut rest[..4 * pixels];
        markesteijn_steps::compute_derivatives(region_a, region_b, width, height);
    }
    tracing::debug!(
        "  Step 3+4 (RGB+deriv): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 5: Build homogeneity maps from derivatives
    // Reads: Region B (drv). Writes: Region C (homo via u8 reinterpret), Region D (threshold).
    // Split storage to get non-overlapping borrows of regions B, C (as u8), and D.
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

    // Step 6: Final blend using homogeneity scores + pre-computed RGB
    // Reads: Region A (rgb_dir), Region C (homo via SATs).
    // Writes: Region B[..3P] (output RGB, overwriting first 3P of drv).
    let t = Instant::now();
    {
        let (region_a, rest) = arena.storage.split_at_mut(12 * pixels);
        let (region_b, region_cd) = rest.split_at_mut(4 * pixels);
        let homo = unsafe {
            let ptr = region_cd.as_ptr() as *const u8;
            std::slice::from_raw_parts(ptr, pixels * 4)
        };
        let output = &mut region_b[..3 * pixels];
        markesteijn_steps::blend_final_from_rgb(region_a, homo, width, height, output);
    }
    tracing::debug!(
        "  Step 6 (blend): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Copy output from Region B into owned Vec
    arena.storage[12 * pixels..12 * pixels + 3 * pixels].to_vec()
}

#[cfg(test)]
mod tests {
    use super::super::{XTransImage, XTransPattern};
    use super::*;

    fn test_pattern() -> XTransPattern {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1],
            [2, 1, 2, 0, 1, 0],
            [1, 2, 1, 1, 0, 1],
            [1, 2, 1, 1, 0, 1],
            [0, 1, 0, 2, 1, 2],
            [1, 0, 1, 1, 2, 1],
        ])
    }

    fn make_xtrans(
        data: &[f32],
        raw_w: usize,
        raw_h: usize,
        w: usize,
        h: usize,
        top: usize,
        left: usize,
    ) -> XTransImage<'_> {
        XTransImage::with_margins(data, raw_w, raw_h, w, h, top, left, test_pattern())
    }

    #[test]
    fn test_markesteijn_output_size() {
        // 24x24 raw, 12x12 active area (needs BORDER=7 margin on each side)
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let data = vec![0.5f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb = demosaic_xtrans_markesteijn(&xtrans);
        assert_eq!(rgb.len(), w * h * 3);
    }

    #[test]
    fn test_markesteijn_uniform_input() {
        let raw_w = 30;
        let raw_h = 30;
        let w = 18;
        let h = 18;
        let data = vec![0.5f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb = demosaic_xtrans_markesteijn(&xtrans);

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
        // Use varied data to exercise all code paths
        let data: Vec<f32> = (0..raw_w * raw_h)
            .map(|i| i as f32 / (raw_w * raw_h) as f32)
            .collect();
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb = demosaic_xtrans_markesteijn(&xtrans);

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
        let data = vec![0.0f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);

        let rgb = demosaic_xtrans_markesteijn(&xtrans);
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
        let data = vec![0.5f32; raw_w * raw_h];
        let pattern = test_pattern();
        let xtrans =
            XTransImage::with_margins(&data, raw_w, raw_h, w, h, top, left, pattern.clone());

        let rgb = demosaic_xtrans_markesteijn(&xtrans);

        // At green pixel positions, the green channel should be the raw value
        for y in 0..h {
            for x in 0..w {
                let raw_y = y + top;
                let raw_x = x + left;
                if pattern.color_at(raw_y, raw_x) == 1 {
                    let g = rgb[(y * w + x) * 3 + 1];
                    assert!(
                        (g - 0.5).abs() < 1e-6,
                        "Green at ({},{}) = {} (expected 0.5)",
                        y,
                        x,
                        g
                    );
                }
            }
        }
    }
}
