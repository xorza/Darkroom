//! Per-pixel background and noise estimates.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::background_mesh::TileGrid;
use crate::background_mesh::spline::{cubic_spline_eval, solve_natural_spline_d2};
use crate::background_mesh::tile_stats::TileComponent;
use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::JobScratchPool;
use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;
use crate::stacking::star_detection::background::simd;
use crate::stacking::star_detection::background::simd::{SegmentRamp, SplineSegment};
use crate::stacking::star_detection::background::sky_noise::SkyNoise;
use crate::stacking::star_detection::background::workspace::InterpolateScratch;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::mask_dilation::dilate_mask;
use crate::stacking::star_detection::resources::DetectionResources;
use crate::stacking::star_detection::threshold_mask::{ThresholdParams, create_threshold_mask};

/// Per-pixel background and noise estimates for an image.
///
/// Used by subsequent pipeline stages for thresholding, centroid computation,
/// and SNR calculation.
#[derive(Debug)]
pub(crate) struct BackgroundEstimate {
    /// Per-pixel background values (sky level).
    pub(crate) background: Buffer2<f32>,
    /// Per-pixel noise (sigma) estimates.
    pub(crate) noise: Buffer2<f32>,
    /// The floor every threshold built from [`Self::noise`] applies to it. See
    /// [`noise_floor_from`] for why it is measured from the frame rather than fixed.
    pub(crate) noise_floor: f32,
}

/// A per-pixel σ is floored at this fraction of the frame's typical tile σ: far enough below the
/// real noise never to bind on a healthy estimate, far enough above zero to keep a degenerate tile
/// from collapsing the threshold onto the sky.
const NOISE_FLOOR_FRACTION: f32 = 1e-4;

/// The per-pixel σ floor for a frame whose typical noise is `scale`.
///
/// Shared so the estimator and the synthetic uniform maps the tests build cannot state the fraction
/// differently.
pub(crate) fn noise_floor_for(scale: f32) -> f32 {
    (scale * NOISE_FLOOR_FRACTION).max(f32::MIN_POSITIVE)
}

/// The frame's own floor for a per-pixel noise estimate.
///
/// The spline interpolates σ between tile centers and can carry it to zero wherever a tile came out
/// flat, which would collapse `bg + σ·noise` onto `bg` and match every pixel above the sky. A fixed
/// constant cannot be that guard: the linear domain is `[0, 1]`, but the span the decoder divided by
/// sets the magnitude, so one frame's entire noise range can sit below a constant sized for another
/// — a 32-bit integer FITS lands near `1e-8` — and the threshold then rejects everything instead.
/// A fraction of this frame's own median tile σ scales with whatever it is handed.
///
/// When no tile has a measurable spread at all — a synthetic or wholly saturated frame — the sky
/// level stands in for it. The floor still has work to do there: the interpolated background map
/// does not reproduce a constant sky exactly, and without a margin above that jitter every pixel
/// clears `bg` and the whole frame labels as one component. The sky carries the frame's magnitude,
/// so a fraction of it stays above the jitter in any domain.
fn noise_floor_from(grid: &TileGrid) -> f32 {
    let stats = grid.stats.pixels();
    let mut sigmas: Vec<f32> = stats
        .iter()
        .map(|tile| tile.sigma)
        .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
        .collect();
    let scale = if sigmas.is_empty() {
        let mut skies: Vec<f32> = stats
            .iter()
            .map(|tile| tile.sky.abs())
            .filter(|sky| sky.is_finite() && *sky > 0.0)
            .collect();
        if skies.is_empty() {
            return f32::MIN_POSITIVE;
        }
        median_mut(&mut skies)
    } else {
        median_mut(&mut sigmas)
    };
    noise_floor_for(scale)
}

impl BackgroundEstimate {
    /// Estimate background and noise for the image.
    ///
    /// Performs tiled sigma-clipped statistics with natural bicubic spline interpolation.
    /// All buffer management is contained within this function.
    pub(crate) fn estimate(
        pixels: &Buffer2<f32>,
        config: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let mut background = resources.acquire_f32();
        let mut noise = resources.acquire_f32();

        let workspace = &mut resources.background;
        let tile_grid = workspace.mesh.compute(
            pixels,
            None,
            config.tile_size,
            config.sigma_clip_iterations,
            true,
        );
        let noise_floor = noise_floor_from(tile_grid);
        interpolate_from_grid(
            tile_grid,
            &mut background,
            &mut noise,
            &workspace.interpolation,
        );

        Self {
            background,
            noise,
            noise_floor,
        }
    }

    /// Refine the estimate by masking the sources it finds and re-estimating the sky around them,
    /// `iterations` times, the mask dilated by `mask_dilation`.
    pub(crate) fn refine(
        &mut self,
        pixels: &Buffer2<f32>,
        config: &BackgroundConfig,
        refinement: Refinement,
        detection_sigma: f32,
        resources: &mut DetectionResources,
    ) {
        let Refinement {
            iterations,
            mask_dilation,
        } = refinement;

        let mut mask = resources.acquire_bit();
        let mut scratch = resources.acquire_bit();

        for _iter in 0..iterations {
            create_object_mask(
                pixels,
                &self.background,
                &self.noise,
                ThresholdParams {
                    sigma: detection_sigma,
                    min_noise: self.noise_floor,
                },
                mask_dilation,
                &mut mask,
                &mut scratch,
            );

            let workspace = &mut resources.background;
            let tile_grid = workspace.mesh.compute(
                pixels,
                Some(&mask),
                config.tile_size,
                config.sigma_clip_iterations,
                true,
            );
            // Re-measured from the refined grid: masking objects out changes the tile σ set the
            // floor is derived from.
            self.noise_floor = noise_floor_from(tile_grid);
            interpolate_from_grid(
                tile_grid,
                &mut self.background,
                &mut self.noise,
                &workspace.interpolation,
            );
        }

        resources.release_bit(scratch);
        resources.release_bit(mask);
    }

    /// Subtract the sky from `pixels` in place, leaving the residual every later stage reads, and
    /// keep only its noise: the background plane goes back to the pool.
    pub(crate) fn subtract_from(
        self,
        pixels: &mut Buffer2<f32>,
        pool: &mut DetectionResources,
    ) -> SkyNoise {
        pixels
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(self.background.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .for_each(|(values, sky)| {
                for (value, &sky) in values.iter_mut().zip(sky) {
                    *value -= sky;
                }
            });
        pool.release_f32(self.background);
        SkyNoise {
            noise: self.noise,
            floor: self.noise_floor,
        }
    }
}

/// The parameters of `BackgroundRefinement::Iterative`, once a refinement is known to run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Refinement {
    pub(crate) iterations: usize,
    pub(crate) mask_dilation: usize,
}

/// Interpolate background map from tile grid into output buffers.
fn interpolate_from_grid(
    grid: &TileGrid,
    background: &mut Buffer2<f32>,
    noise: &mut Buffer2<f32>,
    interpolation: &JobScratchPool<InterpolateScratch>,
) {
    let width = background.width();
    let tiles_x = grid.stats.width();

    background
        .pixels_mut()
        .par_chunks_mut(width)
        .zip(noise.pixels_mut().par_chunks_mut(width))
        .enumerate()
        .for_each_init(
            || interpolation.acquire(),
            |scratch, (y, (bg_row, noise_row))| {
                scratch.resize(tiles_x);
                interpolate_row(bg_row, noise_row, y, grid, scratch);
            },
        );
}

/// Create a mask of pixels that are likely objects (above threshold).
///
/// `output` is used as the mask buffer. `scratch` is used for dilation if needed.
fn create_object_mask(
    pixels: &Buffer2<f32>,
    background: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    threshold: ThresholdParams,
    dilation_radius: usize,
    output: &mut BitBuffer2,
    scratch: &mut BitBuffer2,
) {
    // Create threshold mask using packed SIMD-optimized implementation
    create_threshold_mask(pixels, background, noise, threshold, output);

    dilate_mask(output, dilation_radius, scratch);
}

/// Interpolate an entire row using natural bicubic spline interpolation.
///
/// Two-pass approach matching SExtractor/SEP:
/// 1. Evaluate Y spline at this row for each tile column → node values
/// 2. Solve tridiagonal system in X for second derivatives
/// 3. Evaluate X spline per-pixel using SIMD-accelerated segments
///
/// Uses pre-allocated `scratch` buffers to avoid heap allocations per row.
fn interpolate_row(
    bg_row: &mut [f32],
    noise_row: &mut [f32],
    y: usize,
    grid: &TileGrid,
    scratch: &mut InterpolateScratch,
) {
    let fy = y as f32;
    let width = bg_row.len();
    let tiles_x = grid.stats.width();
    let centers_x = &grid.centers_x;

    let ty0 = grid.find_lower_tile_y(fy);
    let ty1 = (ty0 + 1).min(grid.stats.height() - 1);
    let cy0 = grid.centers_y[ty0];
    let cy1 = grid.centers_y[ty1];
    let hy = cy1 - cy0;
    let ty = if ty1 == ty0 {
        0.0
    } else {
        ((fy - cy0) / hy).clamp(0.0, 1.0)
    };

    // Evaluate Y cubic spline at each tile column
    let node_bg = &mut scratch.node_bg[..tiles_x];
    let node_noise = &mut scratch.node_noise[..tiles_x];

    for tx in 0..tiles_x {
        let f0_bg = grid.stats[(tx, ty0)].sky;
        let f1_bg = grid.stats[(tx, ty1)].sky;
        let d0_bg = grid.d2y(TileComponent::Sky, Vec2us::new(tx, ty0));
        let d1_bg = grid.d2y(TileComponent::Sky, Vec2us::new(tx, ty1));
        node_bg[tx] = cubic_spline_eval(f0_bg, f1_bg, d0_bg, d1_bg, hy, ty);

        let f0_n = grid.stats[(tx, ty0)].sigma;
        let f1_n = grid.stats[(tx, ty1)].sigma;
        let d0_n = grid.d2y(TileComponent::Sigma, Vec2us::new(tx, ty0));
        let d1_n = grid.d2y(TileComponent::Sigma, Vec2us::new(tx, ty1));
        node_noise[tx] = cubic_spline_eval(f0_n, f1_n, d0_n, d1_n, hy, ty);
    }

    let d2x_bg = &mut scratch.d2x_bg[..tiles_x];
    let d2x_noise = &mut scratch.d2x_noise[..tiles_x];

    solve_natural_spline_d2(node_bg, centers_x, d2x_bg, &mut scratch.spline_scratch);
    solve_natural_spline_d2(
        node_noise,
        centers_x,
        d2x_noise,
        &mut scratch.spline_scratch,
    );

    let mut x = 0usize;

    for tx0 in 0..tiles_x {
        let tx1 = (tx0 + 1).min(tiles_x - 1);

        let segment_end = if tx0 + 1 < tiles_x {
            (centers_x[tx0 + 1].floor() as usize).min(width)
        } else {
            width
        };

        if segment_end <= x {
            continue;
        }

        let bg_segment = &mut bg_row[x..segment_end];
        let noise_segment = &mut noise_row[x..segment_end];

        let cx0 = centers_x[tx0];
        let cx1 = centers_x[tx1];

        if tx1 == tx0 {
            // Single tile column — constant fill
            bg_segment.fill(node_bg[tx0]);
            noise_segment.fill(node_noise[tx0]);
        } else {
            let hx = cx1 - cx0;
            let hx2_6 = hx * hx / 6.0;
            let inv_hx = 1.0 / hx;

            simd::interpolate_segment_cubic_simd(
                bg_segment,
                noise_segment,
                SplineSegment {
                    f0: node_bg[tx0],
                    f1: node_bg[tx1],
                    a: hx2_6 * d2x_bg[tx0],
                    b: hx2_6 * d2x_bg[tx1],
                },
                SplineSegment {
                    f0: node_noise[tx0],
                    f1: node_noise[tx1],
                    a: hx2_6 * d2x_noise[tx0],
                    b: hx2_6 * d2x_noise[tx1],
                },
                SegmentRamp {
                    start: (x as f32 - cx0) * inv_hx,
                    step: inv_hx,
                },
            );
        }

        x = segment_end;
        if x >= width {
            break;
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
    use crate::stacking::star_detection::background::sky_noise::SkyNoise;
    use crate::stacking::star_detection::resources::DetectionResources;

    impl BackgroundEstimate {
        /// `pixels` less this background: the residual the stages after the subtraction read.
        pub(crate) fn residual_of(&self, pixels: &Buffer2<f32>) -> Buffer2<f32> {
            let mut residual = pixels.clone();
            for (value, &sky) in residual
                .pixels_mut()
                .iter_mut()
                .zip(self.background.pixels())
            {
                *value -= sky;
            }
            residual
        }

        /// Return both planes to `pool` without subtracting them from anything.
        pub(crate) fn release_to_pool(self, pool: &mut DetectionResources) {
            pool.release_f32(self.background);
            pool.release_f32(self.noise);
        }

        /// This estimate's noise, as [`BackgroundEstimate::subtract_from`] hands it on.
        pub(crate) fn sky_noise(&self) -> SkyNoise {
            SkyNoise {
                noise: self.noise.clone(),
                floor: self.noise_floor,
            }
        }
    }
}
