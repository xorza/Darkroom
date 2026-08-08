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
//! Region B is used as `drv` in Steps 4–5, then as four `u32` scores per pixel in Step 6.
//! Region C is used as `gmin` in Steps 1–2, then reinterpreted as `homo` (u8) in Steps 5–6.
//! Region D is used as `gmax` in Steps 1–2, `threshold` in Step 5, then a `u32` SAT in Step 6.

use common::CancelToken;

use crate::io::raw::alloc_uninit_vec;
use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::markesteijn_steps;
use crate::io::raw::demosaic::{Cancelled, DemosaicMemory};
use crate::math::size2us::Size2us;

/// Number of interpolation directions (4 for 1-pass: H, V, D1, D2).
pub(crate) const NDIR: usize = 4;
const ARENA_WORDS_PER_PIXEL: usize = 18;

pub(crate) fn demosaic_memory(size: Size2us) -> DemosaicMemory {
    let pixels = size.width.saturating_mul(size.height);
    let output_words = pixels.saturating_mul(3);
    let peak_words = pixels.saturating_mul(1 + ARENA_WORDS_PER_PIXEL + 3);
    DemosaicMemory {
        output_bytes: output_words.saturating_mul(std::mem::size_of::<f32>()),
        peak_bytes: peak_words.saturating_mul(std::mem::size_of::<f32>()),
    }
}

/// Preallocated arena for all Markesteijn demosaic working memory.
///
/// Single contiguous allocation with regions that are reused across steps.
/// See module-level docs for the full layout and lifetime diagram.
#[derive(Debug)]
struct DemosaicArena {
    storage: Vec<f32>,
}

#[derive(Debug)]
struct FinalBlendBuffers<'a> {
    green_dir: &'a [f32],
    colors: &'a [[f32; 2]],
    scores: &'a mut [[u32; NDIR]],
    homo: &'a [u8],
    sat: &'a mut [u32],
}

impl DemosaicArena {
    fn new(size: Size2us) -> Self {
        let total = ARENA_WORDS_PER_PIXEL * size.pixel_count();

        // SAFETY: Every element in every region is fully written by parallel passes
        // before being read. See per-step comments in demosaic().
        let storage = unsafe { alloc_uninit_vec::<f32>(total) };

        tracing::debug!(
            "Demosaic arena: {:.1} MB ({} × {} × {} × 4 bytes)",
            (total * 4) as f64 / (1024.0 * 1024.0),
            size.width,
            size.height,
            ARENA_WORDS_PER_PIXEL,
        );

        Self { storage }
    }

    fn final_blend_buffers(&mut self) -> FinalBlendBuffers<'_> {
        const {
            assert!(std::mem::size_of::<f32>() == std::mem::size_of::<u32>());
            assert!(std::mem::align_of::<f32>() >= std::mem::align_of::<u32>());
            assert!(NDIR * std::mem::size_of::<f32>() == std::mem::size_of::<[u32; NDIR]>());
            assert!(std::mem::align_of::<f32>() >= std::mem::align_of::<[u32; NDIR]>());
        }
        debug_assert_eq!(self.storage.len() % ARENA_WORDS_PER_PIXEL, 0);
        let pixels = self.storage.len() / ARENA_WORDS_PER_PIXEL;

        let (regions_ae, regions_bcd) = self.storage.split_at_mut(12 * pixels);
        let (region_b, regions_cd) = regions_bcd.split_at_mut(4 * pixels);
        let (region_c, region_d) = regions_cd.split_at_mut(pixels);
        let green_dir = &regions_ae[..4 * pixels];
        let colors = bytemuck::cast_slice(&regions_ae[4 * pixels..]);
        let scores = bytemuck::cast_slice_mut(region_b);
        let homo = bytemuck::cast_slice(region_c);
        let sat = bytemuck::cast_slice_mut(region_d);

        FinalBlendBuffers {
            green_dir,
            colors,
            scores,
            homo,
            sat,
        }
    }
}

/// Demosaic an X-Trans image using Markesteijn 1-pass algorithm.
///
/// Returns unclipped planar channels `[R, G, B]`, each `width * height`.
pub(crate) fn demosaic(
    xtrans: &XTransImage,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    use std::time::Instant;

    let width = xtrans.active.width;
    let height = xtrans.active.height;
    let pixels = width * height;

    // Build lookup tables
    let hex = HexLookup::new(&xtrans.raw_pattern);
    // Allocate all working memory in one shot
    let mut arena = DemosaicArena::new(xtrans.active);

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
        markesteijn_steps::compute_homogeneity(drv, xtrans.active, homo, region_d);
    }
    tracing::debug!(
        "  Step 5 (homogeneity): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 6: Final blend.
    // Reads: Regions A, E, and C. Reuses B for scores and D for the SAT, and writes planar
    // [R, G, B] directly into the output buffers.
    // SAFETY: blend_final writes every element of each output buffer.
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let mut r = unsafe { alloc_uninit_vec::<f32>(pixels) };
    let mut g = unsafe { alloc_uninit_vec::<f32>(pixels) };
    let mut b = unsafe { alloc_uninit_vec::<f32>(pixels) };
    let t = Instant::now();
    {
        let buffers = arena.final_blend_buffers();
        markesteijn_steps::blend_final(
            xtrans,
            buffers.green_dir,
            buffers.colors,
            buffers.homo,
            buffers.scores,
            buffers.sat,
            &mut r,
            &mut g,
            &mut b,
        );
    }
    tracing::debug!(
        "  Step 6 (blend): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    Ok([r, g, b])
}

#[cfg(test)]
mod tests;
