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

use super::XTransImage;
use super::hex_lookup::HexLookup;
use super::markesteijn_steps;

/// Number of interpolation directions (4 for 1-pass: H, V, D1, D2).
pub(super) const NDIR: usize = 4;

/// Working buffers for Markesteijn demosaicing.
/// Allocated once, buffers are dropped when no longer needed between steps.
#[derive(Debug)]
struct MarkesteijnBuffers {
    /// Per-direction interpolated green. Layout: green_dir[dir * pixels + y * width + x]
    green_dir: Vec<f32>,
    /// Green min bounds for clamping. Layout: gmin[y * width + x]
    gmin: Vec<f32>,
    /// Green max bounds for clamping. Layout: gmax[y * width + x]
    gmax: Vec<f32>,
    /// Per-direction RGB. Layout: rgb_dir[dir * pixels * 3 + y * width * 3 + x * 3 + c]
    /// Computed once in step 3+4, reused in step 6 (avoids expensive R/B recomputation).
    rgb_dir: Vec<f32>,
    /// Per-direction derivative. Layout: drv[dir * pixels + y * width + x]
    drv: Vec<f32>,
    /// Per-direction homogeneity count (0-9, fits u8). Layout: homo[dir * pixels + y * width + x]
    homo: Vec<u8>,
}

impl MarkesteijnBuffers {
    fn new(width: usize, height: usize) -> Self {
        let pixels = width * height;
        Self {
            green_dir: vec![0.0; NDIR * pixels],
            gmin: vec![0.0; pixels],
            gmax: vec![0.0; pixels],
            rgb_dir: vec![0.0; NDIR * pixels * 3],
            drv: vec![0.0; NDIR * pixels],
            homo: vec![0; NDIR * pixels],
        }
    }
}

/// Demosaic an X-Trans image using Markesteijn 1-pass algorithm.
///
/// Returns RGB interleaved pixels: [R0, G0, B0, R1, G1, B1, ...] in 0.0-1.0 range.
pub fn demosaic_xtrans_markesteijn(xtrans: &XTransImage) -> Vec<f32> {
    use std::time::Instant;

    let width = xtrans.width;
    let height = xtrans.height;

    // Build hex neighbor lookup tables
    let hex = HexLookup::new(&xtrans.pattern);

    // Allocate working buffers
    let mut bufs = MarkesteijnBuffers::new(width, height);

    // Step 1: Compute green min/max bounds for non-green pixels
    let t = Instant::now();
    markesteijn_steps::compute_green_minmax(xtrans, &hex, &mut bufs.gmin, &mut bufs.gmax);
    tracing::debug!(
        "  Step 1 (green min/max): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 2: Interpolate green in 4 directions
    let t = Instant::now();
    markesteijn_steps::interpolate_green(xtrans, &hex, &bufs.gmin, &bufs.gmax, &mut bufs.green_dir);
    tracing::debug!(
        "  Step 2 (green interp): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Free gmin/gmax — no longer needed
    drop(bufs.gmin);
    drop(bufs.gmax);

    // Step 3+4: Compute per-direction RGB + YPbPr derivatives
    let t = Instant::now();
    markesteijn_steps::compute_rgb_and_derivatives(
        xtrans,
        &bufs.green_dir,
        &mut bufs.rgb_dir,
        &mut bufs.drv,
    );
    tracing::debug!(
        "  Step 3+4 (RGB+deriv): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Free green_dir — no longer needed (rgb_dir has full RGB)
    drop(bufs.green_dir);

    // Step 5: Build homogeneity maps from derivatives
    let t = Instant::now();
    markesteijn_steps::compute_homogeneity(&bufs.drv, width, height, &mut bufs.homo);
    tracing::debug!(
        "  Step 5 (homogeneity): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Free drv — no longer needed
    drop(bufs.drv);

    // Step 6: Final blend using homogeneity scores + pre-computed RGB
    let t = Instant::now();
    let rgb = markesteijn_steps::blend_final_from_rgb(&bufs.rgb_dir, &bufs.homo, width, height);
    tracing::debug!(
        "  Step 6 (blend): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    rgb
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
