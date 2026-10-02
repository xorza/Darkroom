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
//! Region A holds `green_dir` (4 directions), written in Step 2, read through Step 6.
//! Region E holds directional `[red, blue]` pairs, written in Step 3 and read through Step 6.
//! Region B is used as `drv` in Steps 4–5, then as four `u32` scores per pixel in Step 6.
//! Region C is used as `gmin` in Steps 1–2, then reinterpreted as `homo` (u8) in Steps 5–6.
//! Region D is used as `gmax` in Steps 1–2, `threshold` in Step 5, then a `u32` SAT in Step 6.

use common::CancelToken;

use crate::io::cancelled::Cancelled;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::markesteijn_steps;
use crate::io::raw::demosaic::xtrans::markesteijn_steps::PlanarRgbMut;
use crate::math::size2us::Size2us;

/// Number of interpolation directions (4 for 1-pass: H, V, D1, D2).
pub(crate) const NDIR: usize = 4;
/// Words per pixel of each arena region, in arena order: A, E, B, C, D.
const REGION_WORDS: [usize; 5] = [NDIR, 2 * NDIR, NDIR, 1, 1];
const ARENA_WORDS_PER_PIXEL: usize =
    REGION_WORDS[0] + REGION_WORDS[1] + REGION_WORDS[2] + REGION_WORDS[3] + REGION_WORDS[4];

pub(crate) fn demosaic_memory(size: Size2us) -> DemosaicMemory {
    let pixels = size.width.saturating_mul(size.height);
    let output_words = pixels.saturating_mul(3);
    let peak_words = pixels.saturating_mul(1 + ARENA_WORDS_PER_PIXEL + 3);
    DemosaicMemory {
        output_bytes: output_words.saturating_mul(size_of::<f32>()),
        peak_bytes: peak_words.saturating_mul(size_of::<f32>()),
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

/// The five arena regions the final blend reads and scribbles in, handed over as a
/// set because the arena aliases them out of one allocation and the blend is the
/// only caller that wants them all.
#[derive(Debug)]
pub(super) struct FinalBlendBuffers<'a> {
    pub(super) green_dir: &'a [f32],
    pub(super) colors: &'a [[f32; 2]],
    pub(super) scores: &'a mut [[u32; NDIR]],
    pub(super) homo: &'a [u8],
    pub(super) sat: &'a mut [u32],
}

impl DemosaicArena {
    fn new(size: Size2us) -> Self {
        let total = ARENA_WORDS_PER_PIXEL * size.pixel_count();

        let storage = vec![0.0f32; total];

        tracing::debug!(
            "Demosaic arena: {:.1} MB ({} × {} × {} × 4 bytes)",
            (total * 4) as f64 / (1024.0 * 1024.0),
            size.width,
            size.height,
            ARENA_WORDS_PER_PIXEL,
        );

        Self { storage }
    }

    /// The five regions, split once at their fixed offsets.
    fn regions(&mut self) -> ArenaRegions<'_> {
        debug_assert_eq!(self.storage.len() % ARENA_WORDS_PER_PIXEL, 0);
        let pixels = self.storage.len() / ARENA_WORDS_PER_PIXEL;
        let (a, rest) = self.storage.split_at_mut(REGION_WORDS[0] * pixels);
        let (e, rest) = rest.split_at_mut(REGION_WORDS[1] * pixels);
        let (b, rest) = rest.split_at_mut(REGION_WORDS[2] * pixels);
        let (c, d) = rest.split_at_mut(REGION_WORDS[3] * pixels);
        ArenaRegions { a, e, b, c, d }
    }
}

/// The arena's regions, named as in the module docs. Each step takes the ones it reads and
/// writes, viewed as the type it keeps there at that step.
#[derive(Debug)]
struct ArenaRegions<'a> {
    a: &'a mut [f32],
    e: &'a mut [f32],
    b: &'a mut [f32],
    c: &'a mut [f32],
    d: &'a mut [f32],
}

/// Demosaic an X-Trans image using Markesteijn 1-pass algorithm.
///
/// Returns unclipped planar channels `[R, G, B]`, each `width * height`.
pub(crate) fn demosaic(
    xtrans: &XTransImage<'_>,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    use std::time::Instant;

    let width = xtrans.layout.active.width;
    let height = xtrans.layout.active.height;
    let pixels = width * height;

    // Build lookup tables
    let hex = HexLookup::new(&xtrans.raw_pattern);
    // Allocate all working memory in one shot
    let mut arena = DemosaicArena::new(xtrans.layout.active);

    // Step 1: Compute green min/max bounds for non-green pixels
    // Writes: Region C (gmin), Region D (gmax)
    let t = Instant::now();
    {
        let regions = arena.regions();
        markesteijn_steps::compute_green_minmax(xtrans, &hex, regions.c, regions.d);
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
        let regions = arena.regions();
        markesteijn_steps::interpolate_green(xtrans, &hex, regions.c, regions.d, regions.a);
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
        let regions = arena.regions();
        let colors: &mut [[f32; 2]] = bytemuck::cast_slice_mut(regions.e);
        markesteijn_steps::reconstruct_colors(xtrans, &hex, regions.a, colors);
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
        let regions = arena.regions();
        let colors: &[[f32; 2]] = bytemuck::cast_slice(regions.e);
        markesteijn_steps::compute_derivatives(xtrans, regions.a, colors, regions.b);
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
        let regions = arena.regions();
        // gmin is dead after Step 2, so its words now hold four `u8` homogeneity counts each.
        let homo: &mut [u8] = bytemuck::cast_slice_mut(regions.c);
        markesteijn_steps::compute_homogeneity(regions.b, xtrans.layout.active, homo, regions.d);
    }
    tracing::debug!(
        "  Step 5 (homogeneity): {:.1}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Step 6: Final blend.
    // Reads: Regions A, E, and C. Reuses B for scores and D for the SAT, and writes planar
    // [R, G, B] directly into the output buffers.
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let mut r = vec![0.0f32; pixels];
    let mut g = vec![0.0f32; pixels];
    let mut b = vec![0.0f32; pixels];
    let t = Instant::now();
    {
        let regions = arena.regions();
        markesteijn_steps::blend_final(
            xtrans,
            FinalBlendBuffers {
                green_dir: regions.a,
                colors: bytemuck::cast_slice(regions.e),
                scores: bytemuck::cast_slice_mut(regions.b),
                homo: bytemuck::cast_slice(regions.c),
                sat: bytemuck::cast_slice_mut(regions.d),
            },
            PlanarRgbMut {
                r: &mut r,
                g: &mut g,
                b: &mut b,
            },
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
