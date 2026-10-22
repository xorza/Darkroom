//! Image resampling orchestration for registered frames.

use std::slice;
use std::sync::Arc;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::frame_buffers::{FrameBuffers, NewFrame};
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use crate::image::linear_pixels::LinearPixels;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::registration::resample::flagged_sources::FlagScratch;
use crate::registration::resample::frame_sampler::{
    FrameSampler, RowOutput, SampleMethod, SampleRule, WindowAxes,
};
use crate::registration::resample::row_poss::RowPoss;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::warp_config::WarpConfig;
use crate::registration::warp_transform::WarpTransform;
use crate::scratch_pool::{ScratchPool, Shapeless};

mod flagged_sources;
mod frame_sampler;
mod interior_window;
mod ringing_clamp;
pub(super) mod row_poss;
pub(crate) mod source_image;
mod source_pos;
mod tap_window;
pub(crate) mod warp_config;
mod warp_kernel;

/// Output of [`WarpConfig::warp`](crate::WarpConfig::warp): the aligned image plus per-pixel
/// support and confidence maps.
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
///
/// Every field is written in full by a warp, so a result can take the next frame without clearing.
/// That is the point of reusing them: a fresh plane costs its whole area in first-touch page
/// faults, and the spill tier is done with a set once it is on disk, so a warp stage that reuses
/// one set per worker pays that once instead of once per frame. Worth ~22% of a
/// 16 MP warp (`bench_warp_into_fresh_4k` against `..._reused_4k`), or ~0.1 ms per MiB of plane —
/// the faults are spread across rayon's workers, so this is a fraction of what a single thread
/// would pay for the same pages.
#[derive(Debug)]
pub struct WarpResult {
    /// The aligned image. Its flags are the carried flags — saturation and the flat's floor — of
    /// the source pixels each output pixel's sample gave weight, for a source that carries any.
    pub image: LinearImage,
    pub coverage: Buffer2<f32>,
    pub confidence: Buffer2<f32>,
}

/// What a warp works in beside its result: each worker's row scratch, and each frame's flag planes.
#[derive(Debug, Default)]
pub(crate) struct WarpScratch {
    rows: ScratchPool<RowScratch>,
    flags: ScratchPool<FlagScratch>,
}

impl WarpScratch {
    /// The bytes one warp of a `size` frame on `workers` holds here at the most, its flag planes
    /// and their growth's block ORs: a row's scratch is a few bytes a pixel of one row, which a
    /// plan counts nowhere.
    pub(crate) const fn heap_bytes(size: Size2us, workers: usize) -> usize {
        FlagScratch::heap_bytes(size, workers)
    }
}

/// What one row of a warp works in: the row's source positions, and the tap axes of the pixel it
/// is sampling.
#[derive(Debug, Default)]
pub(crate) struct RowScratch {
    poss: RowPoss,
    axes: WindowAxes,
}

impl Shapeless for RowScratch {}

/// A warped frame on its way to its tier: the resident tier keeps the set as the frame, and the
/// spilled one hands it back for the next frame's warp.
impl FrameBuffers for WarpResult {
    type Image = LinearImage;

    fn image(&self) -> &LinearImage {
        &self.image
    }

    fn quality(&self) -> FrameQuality<&Buffer2<f32>> {
        FrameQuality::Planes {
            coverage: &self.coverage,
            confidence: &self.confidence,
        }
    }

    fn into_new(self) -> NewFrame<LinearImage> {
        let Self {
            image,
            coverage,
            confidence,
        } = self;
        NewFrame {
            image,
            quality: FrameQuality::Planes {
                coverage,
                confidence,
            },
        }
    }
}

/// A warp to run: its transform, and how it samples.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Warp<'a> {
    pub(crate) transform: &'a WarpTransform,
    pub(crate) params: WarpConfig,
}

impl WarpResult {
    /// A set for frames of `dimensions`, every plane zeroed.
    pub(crate) fn new(dimensions: ImageDimensions) -> Self {
        Self {
            image: LinearImage::new(
                LinearPixels::new_zeroed(dimensions),
                None,
                ImageMetadata::default(),
            ),
            coverage: Buffer2::new_default(dimensions.width(), dimensions.height()),
            confidence: Buffer2::new_default(dimensions.width(), dimensions.height()),
        }
    }

    /// Warp `image` by `warp` into this result, overwriting all three planes completely and
    /// replacing the flags and the metadata, in `scratch`.
    ///
    /// One pass over the output rows: each row's source positions are evaluated once — a SIP
    /// correction is the expensive part of a non-linear warp — and every channel, the quality maps
    /// and a flagged frame's validity and flags sample at them.
    pub(crate) fn warp_into(
        &mut self,
        image: &SourceImage<'_>,
        warp: Warp<'_>,
        scratch: &WarpScratch,
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
        let Self {
            image: warped,
            coverage,
            confidence,
        } = self;
        let mut flag_scratch = scratch.flags.acquire();
        // The last frame's flag plane is this frame's, so a worker never holds two.
        flag_scratch.keep(warped.take_flags());
        let mut flag_plane = image
            .flags
            .filter(|flags| flags.contains(QualityFlags::RESAMPLE_CARRIED))
            .map(|_| flag_scratch.output(size));
        // The sampler borrows the flag scratch; it is done with it before the plane is kept.
        {
            let flagged = image
                .flags
                .map(|flags| flag_scratch.prepare(flags, method.window_reach()));
            let rule = SampleRule {
                method,
                border: config.border_value,
            };
            let sampler = FrameSampler::new(rule, image, flagged.as_ref());
            let flag_rows = flag_plane
                .as_mut()
                .map(|plane| UnsafeSendPtr::new(plane.pixels_mut().as_mut_ptr()));

            let warp_row = |scratch: &mut RowScratch,
                            y: usize,
                            channel_rows: &mut [&mut [f32]],
                            coverage_row: &mut [f32],
                            confidence_row: &mut [f32]| {
                scratch.poss.fill(y, warp_transform, size);
                let poss = scratch.poss.poss();
                let flags = flag_rows.map(|rows| unsafe {
                    slice::from_raw_parts_mut(rows.get().add(y * width), width)
                });
                sampler.sample_row(
                    poss,
                    &mut scratch.axes,
                    RowOutput {
                        channels: channel_rows,
                        coverage: coverage_row,
                        confidence: confidence_row,
                        flags,
                    },
                );
            };

            let quality_rows = coverage
                .pixels_mut()
                .par_chunks_mut(width)
                .zip(confidence.pixels_mut().par_chunks_mut(width));
            let mut planes: ArrayVec<&mut Buffer2<f32>, 3> = warped.planes_mut().collect();
            match planes.as_mut_slice() {
                [mono] => mono
                    .pixels_mut()
                    .par_chunks_mut(width)
                    .zip(quality_rows)
                    .enumerate()
                    .for_each_init(
                        || scratch.rows.acquire(),
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
                        || scratch.rows.acquire(),
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
            drop(planes);
            // Set once the planes are written, as writing them drops the noise facts of the
            // samples that were there. An interpolated sample mixes source samples, so one
            // sample's quantization step no longer bounds its noise; the nearest pixel is still
            // one. The mosaic's noise, measured before any warp, stays.
            warped.metadata = image.metadata.clone();
            if method.interpolates() {
                warped.metadata.derived.after_resample();
            }
            warped.set_flat_gain(
                image
                    .flat_gain
                    .map(|gain| Arc::new(gain.warped(|p| warp_transform.apply(p), size))),
            );
        }
        warped.set_flags(flag_plane.and_then(|plane| flag_scratch.finish(plane)));
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
