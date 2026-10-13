//! Image resampling orchestration for registered frames.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;
use std::slice;

use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use crate::image::linear_pixels::LinearPixels;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::registration::resample::flagged_sources::FlaggedSources;
use crate::registration::resample::frame_sampler::{
    FrameSampler, RowOutput, SampleMethod, SampleRule, WindowAxes,
};
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::warp_params::WarpParams;
use crate::registration::transform::warp_transform::WarpTransform;
use crate::scratch_pool::{Scratch, ScratchPool, Shapeless};

mod flagged_sources;
mod frame_sampler;
mod interior_window;
mod kernel;
mod ringing_clamp;
pub(super) mod row_positions;
pub(crate) mod source_image;
mod source_position;
mod tap_window;
pub(crate) mod warp_params;

/// Output of [`WarpTransform::warp`]: the aligned image plus per-pixel support and confidence maps.
///
/// `coverage[p] ∈ [0, 1]` is the share of the interpolation kernel's magnitude that lands on
/// measured source pixels: off the source, and on a pixel flagged a null, a cosmic ray, a defect or
/// its repair, no sample is taken. It is an inclusion mask, not a statistical weight. A bound at
/// saturation or the flat's floor is sampled like any value, and the image's flags carry it to
/// every output pixel whose sample gave it weight.
///
/// `confidence[p]` is the inverse white-noise variance factor implied by the normalized
/// interpolation coefficients: an interpolated sample averages several source pixels, so its white
/// noise is `σ²/confidence`. The combine divides each sample's noise model by it, for rejection and
/// the variance plane, and does not weight the mean by it, as Siril, PixInsight and DSS do not. The
/// smaller variance was bought with resolution, the noise it leaves is correlated between
/// neighbours, and weighting by it would discount the unwarped reference — at confidence 1 — by
/// about 0.79 against a Lanczos3 frame and 0.41 against a bilinear one, and beat with the
/// sub-pixel phase across a rotated field. The price is that the mean's weights no longer make each
/// weighted variance equal, `wᵢvᵢ = 1`, at a warped pixel.
#[derive(Debug)]
pub struct WarpResult {
    pub image: LinearImage,
    pub coverage: Buffer2<f32>,
    pub confidence: Buffer2<f32>,
}

/// What a warp fills: the aligned image — its pixels, the flags it carries and its warped metadata
/// — and the two quality maps.
///
/// Every field is written in full by [`Self::warp_into`], so a set can be leased again for the
/// next frame without clearing. That is the point of pooling them: a fresh plane costs its whole
/// area in first-touch page faults, and the spill tier discards its planes once they are on disk,
/// so a warp stage that reuses one set per worker pays that once instead of once per frame. Worth
/// ~22% of a 16 MP warp (`bench_warp_into_fresh_4k` against `..._reused_4k`), or ~0.1 ms per MiB
/// of plane — the faults are spread across rayon's workers, so this is a fraction of what a single
/// thread would pay for the same pages.
#[derive(Debug)]
pub(crate) struct WarpBuffers {
    /// The aligned image. Its flags are the carried flags of the source pixels each output pixel's
    /// sample gave weight, for a source that carries any; see [`FlaggedSources`].
    pub(crate) image: LinearImage,
    pub(crate) coverage: Buffer2<f32>,
    pub(crate) confidence: Buffer2<f32>,
}

/// What one row of a warp works in: the row's source positions, and the tap axes of the pixel it
/// is sampling.
#[derive(Debug, Default)]
pub(crate) struct RowScratch {
    positions: RowPositions,
    axes: WindowAxes,
}

impl Shapeless for RowScratch {}

/// A set for frames of one shape, reused as it is.
impl Scratch for WarpBuffers {
    type Shape = ImageDimensions;

    fn build(dimensions: ImageDimensions) -> Self {
        Self {
            image: LinearImage {
                metadata: ImageMetadata::default(),
                pixels: LinearPixels::new_zeroed(dimensions),
                flags: None,
            },
            coverage: Buffer2::new_default(dimensions.width(), dimensions.height()),
            confidence: Buffer2::new_default(dimensions.width(), dimensions.height()),
        }
    }

    fn fit(&mut self, dimensions: ImageDimensions) {
        debug_assert!(self.has_shape(dimensions));
    }

    fn has_shape(&self, dimensions: ImageDimensions) -> bool {
        let size = dimensions.size();
        self.image.dimensions() == dimensions
            && Size2us::from(&self.coverage) == size
            && Size2us::from(&self.confidence) == size
    }
}

/// A warp to run: its transform, and how it samples.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Warp<'a> {
    pub(crate) transform: &'a WarpTransform,
    pub(crate) params: WarpParams,
}

impl WarpBuffers {
    /// Warp `image` by `warp` into these buffers, overwriting all three planes completely and
    /// replacing the flags and the metadata; each worker leases its row scratch from `rows`.
    ///
    /// One pass over the output rows: each row's source positions are evaluated once — a SIP
    /// correction is the expensive part of a non-linear warp — and every channel, the quality maps
    /// and a flagged frame's validity and flags sample at them.
    pub(crate) fn warp_into(
        &mut self,
        image: &SourceImage<'_>,
        warp: Warp<'_>,
        rows: &ScratchPool<RowScratch>,
    ) {
        let Warp {
            transform: warp_transform,
            params: config,
        } = warp;
        // Release assert rather than a `Result`: a non-finite border is caller error, not a runtime
        // failure, and it reaches every pixel outside the source footprint — seeding NaN into the
        // combine, where only a debug assert would notice. Once per frame, so the check is free.
        assert!(
            config.border_value.is_finite(),
            "warp border_value must be finite, got {}",
            config.border_value
        );
        let dimensions = image.dimensions;
        assert_eq!(
            self.image.dimensions(),
            dimensions,
            "warp buffers were sized for a different frame"
        );
        let size = dimensions.size();
        let width = size.width;
        let method = SampleMethod::for_frame(config, warp_transform, size);
        let reach = method.reach();
        let flagged = image
            .flags
            .map(|flags| FlaggedSources::new(image, flags, reach));
        let rule = SampleRule {
            method,
            border: config.border_value,
        };
        let sampler = FrameSampler::new(rule, image, flagged.as_ref());
        // The last frame's flags, dropped before this frame's plane is allocated so a worker never
        // holds two.
        self.image.flags = None;
        let mut flag_plane = image
            .flags
            .filter(|flags| flags.contains(QualityFlags::RESAMPLE_CARRIED))
            .map(|_| Buffer2::<u8>::new_default(width, size.height));
        let flag_rows = flag_plane
            .as_mut()
            .map(|plane| UnsafeSendPtr::new(plane.pixels_mut().as_mut_ptr()));

        let warp_row = |scratch: &mut RowScratch,
                        y: usize,
                        channel_rows: &mut [&mut [f32]],
                        coverage_row: &mut [f32],
                        confidence_row: &mut [f32]| {
            scratch.positions.fill(y, warp_transform, size);
            let positions = scratch.positions.positions();
            let flags = flag_rows
                .map(|rows| unsafe { slice::from_raw_parts_mut(rows.get().add(y * width), width) });
            sampler.sample_row(
                positions,
                &mut scratch.axes,
                RowOutput {
                    channels: channel_rows,
                    coverage: coverage_row,
                    confidence: confidence_row,
                    flags,
                },
            );
        };

        let Self {
            image: warped,
            coverage,
            confidence,
        } = self;
        warped.metadata = image
            .metadata
            .clone()
            .warped(|p| warp_transform.apply(p), size);
        let quality_rows = coverage
            .pixels_mut()
            .par_chunks_mut(width)
            .zip(confidence.pixels_mut().par_chunks_mut(width));
        let mut planes: ArrayVec<&mut Buffer2<f32>, 3> = warped.pixels.planes_mut().collect();
        match planes.as_mut_slice() {
            [mono] => mono
                .pixels_mut()
                .par_chunks_mut(width)
                .zip(quality_rows)
                .enumerate()
                .for_each_init(
                    || rows.acquire(),
                    |scratch, (y, (channel, (coverage_row, confidence_row)))| {
                        warp_row(scratch, y, &mut [channel], coverage_row, confidence_row);
                    },
                ),
            [red, green, blue] => red
                .pixels_mut()
                .par_chunks_mut(width)
                .zip(green.pixels_mut().par_chunks_mut(width))
                .zip(blue.pixels_mut().par_chunks_mut(width))
                .zip(quality_rows)
                .enumerate()
                .for_each_init(
                    || rows.acquire(),
                    |scratch, (y, (((red, green), blue), (coverage_row, confidence_row)))| {
                        warp_row(
                            scratch,
                            y,
                            &mut [red, green, blue],
                            coverage_row,
                            confidence_row,
                        );
                    },
                ),
            _ => unreachable!("an image has one channel or three"),
        }
        warped.flags = flag_plane.and_then(PixelFlags::from_buffer);
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
