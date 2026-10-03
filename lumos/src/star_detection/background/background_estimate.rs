//! Per-pixel background and noise estimates.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::background_mesh::TileGrid;
use crate::background_mesh::spline::solve_natural_spline_d2;
use crate::background_mesh::spline::spline_segment::SplineSegment;
use crate::background_mesh::tile_stats::TileComponent;
use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::JobScratchPool;
use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;
use crate::star_detection::background::simd;
use crate::star_detection::background::simd::SegmentRamp;
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::background::workspace::InterpolateScratch;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::detection_plane::{DetectionPlane, PlaneFilters};
use crate::star_detection::mask_dilation::dilate_mask;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::threshold_mask::{ThresholdParams, create_residual_threshold_mask};

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

/// The parameters of `BackgroundRefinement::Iterative`, once a refinement is known to run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Refinement {
    pub(crate) iterations: usize,
    pub(crate) mask_dilation: usize,
    pub(crate) mask_sigma: f32,
}

/// A refined estimate, and the sources it was measured around with the pixels the caller masked.
#[derive(Debug)]
pub(crate) struct RefinedBackground {
    pub(crate) estimate: BackgroundEstimate,
    pub(crate) sources: BitBuffer2,
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
/// constant cannot be that guard: the linear domain is `[0, 1]`, but the span the decoder divided
/// by sets the magnitude, so one frame's entire noise range can sit below a constant sized for
/// another — a 32-bit integer FITS lands near `1e-8` — and the threshold then rejects everything
/// instead. A fraction of this frame's own median tile σ scales with whatever it is handed.
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
    /// Estimate background and noise for the image, leaving out the pixels `mask` sets.
    ///
    /// Performs tiled sigma-clipped statistics with natural bicubic spline interpolation.
    pub(crate) fn estimate(
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        config: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let mut background = resources.acquire_f32();
        let mut noise = resources.acquire_f32();

        let workspace = &mut resources.background;
        let tile_grid = workspace.mesh.compute(
            pixels,
            mask,
            config.tile_size,
            config.sigma_clip_iterations,
            true,
        );
        let noise_floor = noise_floor_from(tile_grid);
        interpolate_from_grid(
            tile_grid,
            Some(&mut background),
            &mut noise,
            &workspace.interpolation,
        );

        Self {
            background,
            noise,
            noise_floor,
        }
    }

    /// The noise of `pixels` alone, by the same mesh, leaving out the pixels `mask` sets: for a
    /// plane whose sky is already out, whose σ is all a threshold needs.
    pub(crate) fn noise_of(
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        config: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> SkyNoise {
        let mut noise = resources.acquire_f32();
        let workspace = &mut resources.background;
        let tile_grid = workspace.mesh.compute(
            pixels,
            mask,
            config.tile_size,
            config.sigma_clip_iterations,
            true,
        );
        let floor = noise_floor_from(tile_grid);
        interpolate_from_grid(tile_grid, None, &mut noise, &workspace.interpolation);
        SkyNoise { noise, floor }
    }

    /// `pixels` less this background, into `residual`.
    pub(crate) fn residual_into(&self, pixels: &Buffer2<f32>, residual: &mut Buffer2<f32>) {
        residual
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(pixels.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .zip(self.background.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .for_each(|((out, values), sky)| {
                for ((out, &value), &sky) in out.iter_mut().zip(values).zip(sky) {
                    *out = value - sky;
                }
            });
    }

    /// Refine the estimate around the sources `refinement` finds: `iterations` times, threshold
    /// the detection plane of the residual at `mask_sigma` of its own σ, dilate the mask by a disk
    /// of `mask_dilation`, add `filters.mask`, and measure the sky again around it. photutils masks
    /// its sources on the convolved data, at 2σ, with a circular footprint.
    pub(crate) fn refine(
        mut self,
        pixels: &Buffer2<f32>,
        refinement: Refinement,
        filters: PlaneFilters<'_>,
        config: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> RefinedBackground {
        let mut sources = resources.acquire_bit();
        let mut scratch = resources.acquire_bit();
        for _ in 0..refinement.iterations {
            let mut residual = resources.acquire_f32();
            self.residual_into(pixels, &mut residual);
            self.release_to_pool(resources);
            let detect = DetectionPlane::from_residual(residual, filters, config, resources);
            create_residual_threshold_mask(
                &detect.values,
                &detect.noise.noise,
                ThresholdParams {
                    sigma: refinement.mask_sigma,
                    min_noise: detect.noise.floor,
                },
                &mut sources,
            );
            detect.release_to_pool(resources);
            dilate_mask(&mut sources, refinement.mask_dilation, &mut scratch);
            if let Some(mask) = filters.mask {
                sources.or_with(mask);
            }
            self = Self::estimate(pixels, Some(&sources), config, resources);
        }
        resources.release_bit(scratch);
        RefinedBackground {
            estimate: self,
            sources,
        }
    }

    /// Return both planes to `pool`.
    pub(crate) fn release_to_pool(self, pool: &mut DetectionResources) {
        pool.release_f32(self.background);
        pool.release_f32(self.noise);
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

/// Interpolate the tile grid into the noise plane, and into the background plane when one is
/// given.
fn interpolate_from_grid(
    grid: &TileGrid,
    background: Option<&mut Buffer2<f32>>,
    noise: &mut Buffer2<f32>,
    interpolation: &JobScratchPool<InterpolateScratch>,
) {
    let width = noise.width();
    let tiles_x = grid.stats.width();
    let sigma_range = grid.sigma_range();
    let background_rows =
        background.map(|background| background.pixels_mut().par_chunks_mut(width));

    let interpolate = |scratch: &mut InterpolateScratch,
                       y: usize,
                       bg_row: Option<&mut [f32]>,
                       noise_row: &mut [f32]| {
        scratch.resize(tiles_x);
        if let Some(bg_row) = bg_row {
            interpolate_row(bg_row, noise_row, y, grid, scratch);
        } else {
            let mut discarded = std::mem::take(&mut scratch.discarded_row);
            discarded.resize(width, 0.0);
            interpolate_row(&mut discarded, noise_row, y, grid, scratch);
            scratch.discarded_row = discarded;
        }
        // The spline overshoots its nodes, and past the outer tiles it extrapolates: on a field
        // whose σ changes fast it reaches below zero. photutils clips its maps to the mesh's range
        // for that reason. Only the noise is clipped here: the sky's extrapolation is what follows
        // a gradient past the outer tiles.
        for sigma in noise_row.iter_mut() {
            *sigma = sigma.clamp(*sigma_range.start(), *sigma_range.end());
        }
    };
    let noise_rows = noise.pixels_mut().par_chunks_mut(width).enumerate();
    match background_rows {
        Some(background_rows) => background_rows.zip(noise_rows).for_each_init(
            || interpolation.acquire(),
            |scratch, (bg_row, (y, noise_row))| interpolate(scratch, y, Some(bg_row), noise_row),
        ),
        None => noise_rows.for_each_init(
            || interpolation.acquire(),
            |scratch, (y, noise_row)| interpolate(scratch, y, None, noise_row),
        ),
    }
}

/// Interpolate an entire row using natural bicubic spline interpolation.
///
/// Two-pass approach matching SExtractor/SEP:
/// 1. Evaluate Y spline at this row for each tile column → node values
/// 2. Solve tridiagonal system in X for second derivatives
/// 3. Evaluate X spline per-pixel using SIMD-accelerated segments
///
/// Uses pre-allocated `scratch` buffers to avoid heap allocations per row.
#[expect(clippy::cast_sign_loss, reason = "a tile centre is non-negative")]
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

    // Past the outer tile centres the end interval's cubic continues, on every side alike, as
    // SEP extrapolates: holding the edge value instead would bend a sky gradient flat there.
    let tiles_y = grid.stats.height();
    let ty0 = grid.find_lower_tile_y(fy).min(tiles_y.saturating_sub(2));
    let ty1 = (ty0 + 1).min(tiles_y - 1);
    let cy0 = grid.centers_y[ty0];
    let cy1 = grid.centers_y[ty1];
    let hy = cy1 - cy0;
    let ty = if ty1 == ty0 { 0.0 } else { (fy - cy0) / hy };

    let node_bg = &mut scratch.node_bg[..tiles_x];
    let node_noise = &mut scratch.node_noise[..tiles_x];

    for tx in 0..tiles_x {
        let f0_bg = grid.stats[(tx, ty0)].sky;
        let f1_bg = grid.stats[(tx, ty1)].sky;
        let d0_bg = grid.d2y(TileComponent::Sky, Vec2us::new(tx, ty0));
        let d1_bg = grid.d2y(TileComponent::Sky, Vec2us::new(tx, ty1));
        node_bg[tx] = SplineSegment::new(f0_bg, f1_bg, d0_bg, d1_bg, hy).eval(ty);

        let f0_n = grid.stats[(tx, ty0)].sigma;
        let f1_n = grid.stats[(tx, ty1)].sigma;
        let d0_n = grid.d2y(TileComponent::Sigma, Vec2us::new(tx, ty0));
        let d1_n = grid.d2y(TileComponent::Sigma, Vec2us::new(tx, ty1));
        node_noise[tx] = SplineSegment::new(f0_n, f1_n, d0_n, d1_n, hy).eval(ty);
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

    if tiles_x == 1 {
        bg_row.fill(node_bg[0]);
        noise_row.fill(node_noise[0]);
        return;
    }

    // Each interval takes the pixels below its upper centre; the first and last run on to the
    // row's ends.
    let mut x = 0usize;
    for tx0 in 0..tiles_x - 1 {
        let tx1 = tx0 + 1;
        let segment_end = if tx1 < tiles_x - 1 {
            (centers_x[tx1].ceil() as usize).min(width)
        } else {
            width
        };
        let cx0 = centers_x[tx0];
        let hx = centers_x[tx1] - cx0;
        let inv_hx = 1.0 / hx;

        simd::interpolate_segment_cubic(
            &mut bg_row[x..segment_end],
            &mut noise_row[x..segment_end],
            SplineSegment::new(node_bg[tx0], node_bg[tx1], d2x_bg[tx0], d2x_bg[tx1], hx),
            SplineSegment::new(
                node_noise[tx0],
                node_noise[tx1],
                d2x_noise[tx0],
                d2x_noise[tx1],
                hx,
            ),
            SegmentRamp {
                start: (x as f32 - cx0) * inv_hx,
                step: inv_hx,
            },
        );
        x = segment_end;
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::star_detection::background::background_estimate::BackgroundEstimate;
    use crate::star_detection::background::sky_noise::SkyNoise;

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

        /// This estimate's noise, as [`BackgroundEstimate::subtract_from`] hands it on.
        pub(crate) fn sky_noise(&self) -> SkyNoise {
            SkyNoise {
                noise: self.noise.clone(),
                floor: self.noise_floor,
            }
        }
    }
}
