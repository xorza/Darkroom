//! Markesteijn 1-pass demosaicing for X-Trans sensors.
//!
//! Implements Frank Markesteijn's directional interpolation algorithm with
//! homogeneity-based direction selection. Produces significantly better quality
//! than bilinear interpolation, especially for star profiles in astrophotography.
//!
//! The algorithm:
//! 1. Interpolates green in 4 directions using weighted hexagonal neighbors
//! 2. Computes perceptual derivatives (recomputing RGB on-the-fly)
//! 3. Builds homogeneity maps to identify the best direction(s) per pixel
//! 4. Blends the best directions into the final RGB output (recomputing RGB on-the-fly)
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
//! [ Region A: green_dir (4P) | Region B: drv/output (4P) | C: gmin/homo (P) | D: gmax/threshold (P) ]
//! Total: 10P f32 elements, where P = width × height
//! ```
//!
//! Region A holds green_dir (4 directions), written in Step 2, read through Step 4.
//! Region B is used as `drv` in Steps 3–4, then output (3P) is written in Step 5.
//! Region C is used as `gmin` in Steps 1–2, then reinterpreted as `homo` (u8) in Steps 4–5.
//! Region D is used as `gmax` in Steps 1–2, then reused as `threshold` in Step 4.
//!
//! RGB is never materialized — it's recomputed on-the-fly in Steps 3 and 5 from green_dir,
//! eliminating the 12P rgb_dir buffer (~1.1 GB for 6032×4028).

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
        let total = 10 * pixels; // 4P + 4P + P + P

        // SAFETY: Every element in every region is fully written by parallel passes
        // before being read. See per-step comments in demosaic_xtrans_markesteijn().
        let storage = unsafe { crate::raw::alloc_uninit_vec::<f32>(total) };

        tracing::debug!(
            "Demosaic arena: {:.1} MB ({} × {} × 10 × 4 bytes)",
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

    // Build lookup tables
    let hex = HexLookup::new(&xtrans.pattern);
    let color_lookup = markesteijn_steps::ColorInterpLookup::new(&xtrans.pattern);

    // Allocate all working memory in one shot
    let mut arena = DemosaicArena::new(width, height);

    // Step 1: Compute green min/max bounds for non-green pixels
    // Writes: Region C (gmin), Region D (gmax)
    let t = Instant::now();
    {
        let (before_d, region_d) = arena.storage.split_at_mut(9 * pixels);
        let region_c = &mut before_d[8 * pixels..];
        markesteijn_steps::compute_green_minmax(xtrans, &hex, region_c, region_d);
    }
    tracing::debug!(
        "  Step 1 (green min/max): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 2: Interpolate green in 4 directions
    // Reads: Region C (gmin), Region D (gmax). Writes: Region A (green_dir).
    let t = Instant::now();
    {
        let (region_a, cd) = arena.storage.split_at_mut(8 * pixels);
        let region_a = &mut region_a[..4 * pixels];
        let (region_c, region_d) = cd.split_at(pixels);
        markesteijn_steps::interpolate_green(xtrans, &hex, region_c, region_d, region_a);
    }
    tracing::debug!(
        "  Step 2 (green interp): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 3: Compute YPbPr derivatives (recomputing RGB on-the-fly from green_dir)
    // Reads: Region A (green_dir). Writes: Region B (drv).
    let t = Instant::now();
    {
        let (region_a, rest) = arena.storage.split_at_mut(4 * pixels);
        let region_b = &mut rest[..4 * pixels];
        markesteijn_steps::compute_derivatives(xtrans, region_a, &color_lookup, region_b);
    }
    tracing::debug!(
        "  Step 3 (derivatives): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 4: Build homogeneity maps from derivatives
    // Reads: Region B (drv). Writes: Region C (homo via u8 reinterpret), Region D (threshold).
    let t = Instant::now();
    {
        let (before_d, region_d) = arena.storage.split_at_mut(9 * pixels);
        let (before_c, region_c) = before_d.split_at_mut(8 * pixels);
        let drv = &before_c[4 * pixels..];
        // SAFETY: Region C (f32 at [8P..9P]) reinterpreted as u8 for homo.
        // gmin data is dead after Step 2. f32 alignment (4) satisfies u8 alignment (1).
        let homo =
            unsafe { std::slice::from_raw_parts_mut(region_c.as_mut_ptr() as *mut u8, pixels * 4) };
        markesteijn_steps::compute_homogeneity(drv, width, height, homo, region_d);
    }
    tracing::debug!(
        "  Step 4 (homogeneity): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 5: Final blend (recomputing RGB on-the-fly from green_dir + homogeneity)
    // Reads: Region A (green_dir), Region C (homo via SATs).
    // Writes: Region B[..3P] (output RGB, overwriting first 3P of dead drv).
    let t = Instant::now();
    {
        let (region_a, rest) = arena.storage.split_at_mut(4 * pixels);
        let (region_b, region_cd) = rest.split_at_mut(4 * pixels);
        let homo = unsafe {
            let ptr = region_cd.as_ptr() as *const u8;
            std::slice::from_raw_parts(ptr, pixels * 4)
        };
        let output = &mut region_b[..3 * pixels];
        markesteijn_steps::blend_final(
            xtrans,
            region_a,
            &color_lookup,
            homo,
            width,
            height,
            output,
        );
    }
    tracing::debug!(
        "  Step 5 (blend): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Copy output from Region B into owned Vec
    arena.storage[4 * pixels..4 * pixels + 3 * pixels].to_vec()
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

    const TEST_INV_RANGE: f32 = 1.0 / 65535.0;

    fn to_u16(val: f32) -> u16 {
        (val * 65535.0).round() as u16
    }

    fn make_xtrans(
        data: &[u16],
        raw_w: usize,
        raw_h: usize,
        w: usize,
        h: usize,
        top: usize,
        left: usize,
    ) -> XTransImage<'_> {
        XTransImage::with_margins(
            data,
            raw_w,
            raw_h,
            w,
            h,
            top,
            left,
            test_pattern(),
            [0.0; 3],
            TEST_INV_RANGE,
            [1.0; 3],
        )
    }

    #[test]
    fn test_markesteijn_output_size() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let data = vec![to_u16(0.5); raw_w * raw_h];
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
        let data = vec![to_u16(0.5); raw_w * raw_h];
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
        let data: Vec<u16> = (0..raw_w * raw_h)
            .map(|i| to_u16(i as f32 / (raw_w * raw_h) as f32))
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
        let data = vec![0u16; raw_w * raw_h];
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
            [1.0; 3],
        );

        let rgb = demosaic_xtrans_markesteijn(&xtrans);

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
