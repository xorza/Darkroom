//! Image resampling orchestration for registered frames.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;
use std::slice;

use crate::concurrency::{JobScratchPool, UnsafeSendPtr};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::registration::config::WarpParams;
use crate::registration::resample::frame_sampler::{
    FrameSampler, RowOutput, SampleMethod, WindowAxes,
};
use crate::registration::resample::masked_sources::MaskedSources;
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::transform::WarpTransform;

mod frame_sampler;
mod interior_window;
mod kernel;
mod masked_sources;
mod ringing_clamp;
mod row_positions;
pub(crate) mod source_image;
mod source_position;
mod tap_window;

/// Output of [`warp`]: the aligned image plus per-pixel support and confidence maps.
///
/// `coverage[p] ∈ [0, 1]` is the share of the interpolation kernel's magnitude that lands on real
/// source pixels. It is an inclusion mask, not a statistical weight. `confidence[p]` is the inverse
/// white-noise variance implied by the normalized interpolation coefficients.
#[derive(Debug)]
pub struct WarpResult {
    pub image: LinearImage,
    pub coverage: Buffer2<f32>,
    pub confidence: Buffer2<f32>,
}

/// Warp an image to align with the reference frame.
///
/// The `WarpTransform` bundles the linear transform with optional SIP distortion
/// correction. Use `result.warp_transform()` to obtain one from a `RegistrationResult`,
/// or `WarpTransform::new(transform)` for a plain transform.
///
/// The output has the same dimensions and metadata as `image`; every output pixel
/// is produced by inverse-mapping, so no input pixels are carried over. Returns a
/// [`WarpResult`] carrying the aligned image and its quality maps (see that type).
///
/// The maps are always produced: the pipeline's only caller needs both, so there is no
/// unconditional cost to skip. They are geometry, not pixels, so each row's maps are written once,
/// from the same source positions every channel of that row samples at.
///
/// # Arguments
/// * `image` - The source (target) image to warp
/// * `warp_transform` - Combined transform + optional SIP correction
/// * `config` - Configuration for interpolation method
///
/// # Example
///
/// ```no_run
/// use lumos::{LinearImage, RegistrationConfig, Star, register, warp};
///
/// # fn example(ref_stars: &[Star], target_stars: &[Star], target_image: &LinearImage)
/// # -> Result<(), lumos::RegistrationError> {
/// let config = RegistrationConfig::default();
/// let result = register(ref_stars, target_stars, &config)?;
/// let aligned = warp(target_image, &result.warp_transform(), config.warp).image;
/// # Ok(())
/// # }
/// ```
pub fn warp(image: &LinearImage, warp_transform: &WarpTransform, config: WarpParams) -> WarpResult {
    let mut buffers = WarpBuffers::new(image.dimensions());
    buffers.warp_into(&SourceImage::of(image), warp_transform, config);
    WarpResult {
        image: LinearImage {
            metadata: image.metadata.clone(),
            pixels: buffers.pixels,
            flags: buffers.flags,
        },
        coverage: buffers.coverage,
        confidence: buffers.confidence,
    }
}

/// The three planes a warp fills — the aligned pixels and the two quality maps — and the row
/// scratch it fills them through.
///
/// Every plane is written in full by [`Self::warp_into`], so a set can be handed straight back for
/// the next frame without clearing. That is the point of naming them: a fresh plane costs its
/// whole area in first-touch page faults, and the spill tier discards its planes once they are on
/// disk, so a warp stage that reuses one set per worker pays that once instead of once per frame.
/// Worth ~22% of a 16 MP warp (`bench_warp_into_fresh_4k` against `..._reused_4k`), or ~0.1 ms per
/// MiB of plane — the faults are spread across rayon's workers, so this is a fraction of what a
/// single thread would pay for the same pages. The row scratch is kept for the same reason.
#[derive(Debug)]
pub(crate) struct WarpBuffers {
    pub(crate) pixels: LinearPixels,
    pub(crate) coverage: Buffer2<f32>,
    pub(crate) confidence: Buffer2<f32>,
    /// The source's flags at each output pixel, for a source that carries any but `NO_DATA`: every
    /// flag of a source pixel the kernel can read there. The source's nulls are in `coverage`
    /// instead. A null spreads over the kernel footprint of every output pixel that reached it, so
    /// what comes out is a fraction per pixel rather than a yes-or-no, and the combine gates on
    /// that fraction.
    pub(crate) flags: Option<PixelFlags>,
    pub(crate) rows: JobScratchPool<RowScratch>,
}

/// What one row of a warp works in: the row's source positions, and the tap axes of the pixel it
/// is sampling.
#[derive(Debug, Default)]
pub(crate) struct RowScratch {
    positions: RowPositions,
    axes: WindowAxes,
}

impl WarpBuffers {
    pub(crate) fn new(dimensions: ImageDimensions) -> Self {
        Self {
            pixels: LinearPixels::new_zeroed(dimensions),
            coverage: Buffer2::new_default(dimensions.width(), dimensions.height()),
            confidence: Buffer2::new_default(dimensions.width(), dimensions.height()),
            flags: None,
            rows: JobScratchPool::default(),
        }
    }

    pub(crate) fn dimensions(&self) -> ImageDimensions {
        self.pixels.dimensions()
    }

    /// Warp `image` into these buffers, overwriting all three completely.
    ///
    /// One pass over the output rows: each row's source positions are evaluated once — a SIP
    /// correction is the expensive part of a non-linear warp — and every channel, the quality maps
    /// and a masked frame's validity sample at them.
    pub(crate) fn warp_into(
        &mut self,
        image: &SourceImage<'_>,
        warp_transform: &WarpTransform,
        config: WarpParams,
    ) {
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
            self.dimensions(),
            dimensions,
            "warp buffers were sized for a different frame"
        );
        let size = dimensions.size();
        let width = size.width;
        let method = SampleMethod::for_frame(config, warp_transform, size);
        let reach = method.reach();
        // The source declared pixels with no measurement, so every output pixel is reconstructed
        // from its taps that hold data, and its coverage is the share of the kernel they carry.
        let masked = image
            .flags
            .as_deref()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
            .map(|flags| MaskedSources::new(image, flags, reach));
        let sampler = FrameSampler::new(method, image, masked.as_ref(), config.border_value);
        // Grown by the kernel's reach once, so each output pixel reads one byte at its source cell
        // rather than one per tap: the cell's flags then cover every source pixel its window reads.
        let source_flags = image
            .flags
            .as_deref()
            .filter(|flags| flags.contains_other_than(QualityFlags::NO_DATA))
            .and_then(|flags| flags.without(QualityFlags::NO_DATA))
            .map(|mut flags| {
                flags.dilate_window(reach, QualityFlags::default());
                flags
            });
        let mut flag_plane = source_flags
            .as_ref()
            .map(|_| Buffer2::<u8>::new_default(width, size.height));
        let flag_rows = flag_plane
            .as_mut()
            .map(|plane| UnsafeSendPtr::new(plane.pixels_mut().as_mut_ptr()));

        let warp_row = |scratch: &mut RowScratch,
                        y: usize,
                        channel_rows: &mut [&mut [f32]],
                        coverage_row: &mut [f32],
                        confidence_row: &mut [f32]| {
            scratch.positions.fill(y, width, warp_transform, size);
            let positions = scratch.positions.positions();
            if let (Some(source), Some(rows)) = (&source_flags, flag_rows) {
                // SAFETY: each output row is written by exactly one call, at its own offset.
                let row = unsafe { slice::from_raw_parts_mut(rows.get().add(y * width), width) };
                for (flag, position) in row.iter_mut().zip(positions) {
                    *flag = position.map_or(0, |p| {
                        // A footprint edge at −½ floors to cell −1.
                        let x = (p.cell_x.max(0).unsigned_abs() as usize).min(size.width - 1);
                        let y = (p.cell_y.max(0).unsigned_abs() as usize).min(size.height - 1);
                        source.byte(y * width + x)
                    });
                }
            }
            sampler.sample_row(
                positions,
                &mut scratch.axes,
                RowOutput {
                    channels: channel_rows,
                    coverage: coverage_row,
                    confidence: confidence_row,
                },
            );
        };

        let Self {
            pixels,
            coverage,
            confidence,
            rows,
            ..
        } = self;
        let quality_rows = coverage
            .pixels_mut()
            .par_chunks_mut(width)
            .zip(confidence.pixels_mut().par_chunks_mut(width));
        let mut planes: ArrayVec<&mut Buffer2<f32>, 3> = pixels.planes_mut().collect();
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
        self.flags = flag_plane.and_then(PixelFlags::from_buffer);
    }
}

#[cfg(test)]
pub(super) mod internals {
    use imaginarium::Buffer2;

    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::linear::LinearImage;
    use crate::registration::config::WarpParams;
    use crate::registration::resample;
    use crate::registration::transform::WarpTransform;

    /// One plane warped on its own — the shape the kernel tests and the plane benches compare.
    pub(crate) fn warp_plane(
        input: &Buffer2<f32>,
        output: &mut Buffer2<f32>,
        transform: &WarpTransform,
        params: WarpParams,
    ) {
        let image = LinearImage::from_pixels(
            ImageDimensions::new((input.width(), input.height()), 1),
            input.pixels().to_vec(),
        );
        let warped = resample::warp(&image, transform, params);
        output
            .pixels_mut()
            .copy_from_slice(warped.image.channel(0).pixels());
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
