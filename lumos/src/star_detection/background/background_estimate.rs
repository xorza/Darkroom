//! Per-pixel background and noise estimates.

use std::mem;

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::background_mesh::BackgroundMesh;
use crate::background_mesh::spline::natural_spline::NaturalSpline;
use crate::background_mesh::spline::spline_segment::{SplineNode, SplineSegment};
use crate::background_mesh::tile_stats::TileComponent;
use crate::bit_buffer2::BitBuffer2;
use crate::image::masked_plane::MaskedPlane;
use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::simd::Kernel;
use crate::star_detection::background::simd::{InterpolateSegment, SegmentRamp};
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::background::workspace::{InterpolateScratch, InterpolateShape};
use crate::star_detection::detection_plane::{DetectionPlane, PlaneSpec};
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::background_config::{
    BackgroundConfig, BackgroundRefinement,
};

/// Per-pixel background and noise estimates for an image.
///
/// Used by subsequent pipeline stages for thresholding, centroid computation,
/// and SNR calculation.
#[derive(Debug)]
pub(crate) struct BackgroundEstimate {
    /// Per-pixel background values (sky level).
    pub(crate) background: Pooled<Buffer2<f32>>,
    /// Per-pixel noise (sigma) estimates.
    pub(crate) noise: Pooled<Buffer2<f32>>,
    /// The floor every threshold built from [`Self::noise`] applies to it. See
    /// [`noise_floor_from`] for why it is measured from the frame rather than fixed.
    pub(crate) noise_floor: f32,
}

/// A refined estimate, and the sources it was measured around with the pixels the caller masked,
/// none when the configuration refines nothing.
#[derive(Debug)]
pub(crate) struct RefinedBackground {
    pub(crate) estimate: BackgroundEstimate,
    pub(crate) sources: Option<Pooled<BitBuffer2>>,
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
pub(super) fn noise_floor_from(grid: &BackgroundMesh, values: &mut Vec<f32>) -> f32 {
    let stats = grid.stats.pixels();
    values.clear();
    values.extend(
        stats
            .iter()
            .map(|tile| tile.sigma)
            .filter(|sigma| sigma.is_finite() && *sigma > 0.0),
    );
    if values.is_empty() {
        values.extend(
            stats
                .iter()
                .map(|tile| tile.sky.abs())
                .filter(|sky| sky.is_finite() && *sky > 0.0),
        );
        if values.is_empty() {
            return f32::MIN_POSITIVE;
        }
    }
    noise_floor_for(median_mut(values))
}

impl BackgroundEstimate {
    /// Estimate background and noise for the plane, leaving out the pixels its mask sets.
    ///
    /// Performs tiled sigma-clipped statistics with natural bicubic spline interpolation.
    pub(crate) fn estimate(
        plane: MaskedPlane<'_>,
        config: &BackgroundConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let MaskedPlane { pixels, mask } = plane;
        let mut background = resources.acquire_f32();
        let mut noise = resources.acquire_f32();

        let workspace = &mut resources.background;
        let tile_grid = workspace.mesh.compute(pixels, mask, config.mesh_params());
        let noise_floor = noise_floor_from(tile_grid, &mut resources.values);
        interpolate_from_grid(
            tile_grid,
            Planes {
                background: Some(&mut background),
                noise: &mut noise,
            },
            &mut workspace.interpolation,
        );

        Self {
            background,
            noise,
            noise_floor,
        }
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

    /// Refine the estimate around its sources as `spec.background`'s refinement asks:
    /// `iterations` times, threshold the detection plane `spec` makes of the residual at
    /// `mask_sigma` of its own σ, dilate the mask by a disk of `mask_dilation`, add `spec.mask`,
    /// and measure the sky again around it. photutils masks its sources on the convolved data, at
    /// 2σ, with a circular footprint. Without a refinement the estimate comes back as it is, with
    /// no sources.
    ///
    /// `iterations` is at least one by its type, and it has to be: the loop is what writes the
    /// source mask this returns, and a mask leased from the pool holds whatever its last holder
    /// left.
    pub(crate) fn refine(
        mut self,
        pixels: &Buffer2<f32>,
        spec: PlaneSpec<'_>,
        resources: &mut DetectionResources,
    ) -> RefinedBackground {
        let BackgroundRefinement::Iterative(refinement) = spec.background.refinement else {
            return RefinedBackground {
                estimate: self,
                sources: None,
            };
        };
        let mut sources = resources.acquire_bit();
        let mut scratch = resources.acquire_bit();
        for _ in 0..refinement.iterations.get() {
            let mut residual = resources.acquire_f32();
            self.residual_into(pixels, &mut residual);
            drop(self);
            let detect = DetectionPlane::from_residual(residual, spec, resources);
            detect.threshold(refinement.mask_sigma, &mut sources);
            drop(detect);
            sources.dilate(refinement.mask_dilation, &mut scratch);
            if let Some(mask) = spec.mask {
                sources.or_with(mask);
            }
            let plane = MaskedPlane {
                pixels,
                mask: Some(&sources),
            };
            self = Self::estimate(plane, spec.background, resources);
        }
        RefinedBackground {
            estimate: self,
            sources: Some(sources),
        }
    }

    /// Subtract the sky from `pixels` in place, leaving the residual every later stage reads, and
    /// keep only its noise: the background plane goes back to the pool.
    pub(crate) fn subtract_from(self, pixels: &mut Buffer2<f32>) -> SkyNoise {
        pixels
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(self.background.pixels().par_chunks(SAMPLES_PER_BLOCK))
            .for_each(|(values, sky)| {
                for (value, &sky) in values.iter_mut().zip(sky) {
                    *value -= sky;
                }
            });
        SkyNoise {
            noise: self.noise,
            floor: self.noise_floor,
        }
    }
}

/// The planes an interpolation of the tile grid writes: the noise, and the background when it is
/// kept.
#[derive(Debug)]
pub(super) struct Planes<'a> {
    pub(super) background: Option<&'a mut Buffer2<f32>>,
    pub(super) noise: &'a mut Buffer2<f32>,
}

/// Interpolate the tile grid into `planes`.
pub(super) fn interpolate_from_grid(
    grid: &BackgroundMesh,
    planes: Planes<'_>,
    interpolation: &mut ScratchPool<InterpolateScratch>,
) {
    let Planes { background, noise } = planes;
    let width = noise.width();
    interpolation.reshape(InterpolateShape {
        tiles_x: grid.stats.width(),
        width,
    });
    let sigma_range = grid.sigma_range();
    let background_rows =
        background.map(|background| background.pixels_mut().par_chunks_mut(width));

    let interpolate = |scratch: &mut InterpolateScratch,
                       y: usize,
                       bg_row: Option<&mut [f32]>,
                       noise_row: &mut [f32]| {
        if let Some(bg_row) = bg_row {
            interpolate_row(
                Row {
                    y,
                    background: bg_row,
                    noise: noise_row,
                },
                grid,
                scratch,
            );
        } else {
            let mut discarded = mem::take(&mut scratch.discarded_row);
            interpolate_row(
                Row {
                    y,
                    background: &mut discarded,
                    noise: noise_row,
                },
                grid,
                scratch,
            );
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
    let interpolation = &*interpolation;
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

/// One image row of both planes an interpolation writes.
#[derive(Debug)]
struct Row<'a> {
    y: usize,
    background: &'a mut [f32],
    noise: &'a mut [f32],
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
fn interpolate_row(row: Row<'_>, grid: &BackgroundMesh, scratch: &mut InterpolateScratch) {
    let Row {
        y,
        background: bg_row,
        noise: noise_row,
    } = row;
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
        let node = |component: TileComponent, ty: usize| SplineNode {
            value: grid.stats[(tx, ty)].get(component),
            d2: grid.d2y(component, Vec2us::new(tx, ty)),
        };
        let column = |component| SplineSegment::new(node(component, ty0), node(component, ty1), hy);
        node_bg[tx] = column(TileComponent::Sky).eval(ty);
        node_noise[tx] = column(TileComponent::Sigma).eval(ty);
    }

    let d2x_bg = &mut scratch.d2x_bg[..tiles_x];
    let d2x_noise = &mut scratch.d2x_noise[..tiles_x];

    NaturalSpline {
        values: node_bg,
        centers: centers_x,
    }
    .second_derivatives(d2x_bg, &mut scratch.spline_scratch);
    NaturalSpline {
        values: node_noise,
        centers: centers_x,
    }
    .second_derivatives(d2x_noise, &mut scratch.spline_scratch);

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
        let segment = |values: &[f32], d2: &[f32]| {
            let node = |tx: usize| SplineNode {
                value: values[tx],
                d2: d2[tx],
            };
            SplineSegment::new(node(tx0), node(tx1), hx)
        };

        InterpolateSegment {
            bg_out: &mut bg_row[x..segment_end],
            noise_out: &mut noise_row[x..segment_end],
            bg: segment(node_bg, d2x_bg),
            noise: segment(node_noise, d2x_noise),
            ramp: SegmentRamp {
                start: (x as f32 - cx0) * inv_hx,
                step: inv_hx,
            },
        }
        .dispatch();
        x = segment_end;
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::math::size2us::Size2us;
    use crate::scratch_pool::Pooled;
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
            let size = Size2us::new(self.noise.width(), self.noise.height());
            SkyNoise {
                noise: Pooled::loose(Buffer2::clone(&self.noise), size),
                floor: self.noise_floor,
            }
        }
    }
}
