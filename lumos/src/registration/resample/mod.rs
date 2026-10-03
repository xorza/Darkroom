//! Image resampling orchestration for registered frames.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::concurrency::JobScratchPool;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::pixel_flags::Flags;
use crate::registration::config::WarpParams;
use crate::registration::resample::masked_warp::MaskedSources;
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::transform::WarpTransform;

mod kernel;
mod masked_warp;
mod quality;
mod row;
mod row_positions;
mod source_position;

/// Output of [`warp`]: the aligned image plus per-pixel support and confidence maps.
///
/// `coverage[p] ∈ [0, 1]` is the fraction of interpolation-kernel magnitude supported by real
/// source pixels. It is a geometric inclusion mask, not a statistical weight. `confidence[p]` is
/// the inverse white-noise variance implied by the normalized interpolation coefficients.
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
/// ```ignore
/// use lumos::{RegistrationConfig, register, warp};
///
/// let result = register(&ref_stars, &target_stars, &RegistrationConfig::default())?;
/// let config = RegistrationConfig::default();
/// let aligned = warp(&target_image, &result.warp_transform(), &config.warp).image;
/// ```
pub fn warp(image: &LinearImage, warp_transform: &WarpTransform, config: WarpParams) -> WarpResult {
    let mut buffers = WarpBuffers::new(image.dimensions());
    buffers.warp_into(image, warp_transform, config);
    WarpResult {
        image: LinearImage {
            metadata: image.metadata.clone(),
            pixels: buffers.pixels,
            // The source's nulls are in `coverage` now, not here. A null spreads over the kernel
            // footprint of every output pixel that reached it, so what comes out is a fraction per
            // pixel rather than the yes-or-no a mask can hold — and the combine gates on that
            // fraction. A mask here would be a second, coarser record able only to disagree with
            // it.
            flags: None,
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
    pub(crate) rows: JobScratchPool<RowScratch>,
}

/// What one row of a warp works in: the row's source positions, and for a masked frame the row of
/// support every channel divides by.
#[derive(Debug, Default)]
pub(crate) struct RowScratch {
    positions: RowPositions,
    support: Vec<f32>,
}

impl WarpBuffers {
    pub(crate) fn new(dimensions: ImageDimensions) -> Self {
        Self {
            pixels: LinearPixels::new_zeroed(dimensions),
            coverage: Buffer2::new_default(dimensions.width(), dimensions.height()),
            confidence: Buffer2::new_default(dimensions.width(), dimensions.height()),
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
        image: &LinearImage,
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
        let dimensions = image.dimensions();
        assert_eq!(
            self.dimensions(),
            dimensions,
            "warp buffers were sized for a different frame"
        );
        let size = dimensions.size();
        let width = size.width;
        // The source declared pixels with no measurement, so every output pixel is reconstructed
        // from its surviving taps and the maps are reduced by how many of them there were.
        let masked = image
            .flags
            .as_ref()
            .filter(|flags| flags.contains(Flags::NO_DATA))
            .map(|flags| MaskedSources::new(image, flags));

        let warp_row = |scratch: &mut RowScratch,
                        y: usize,
                        channel_rows: &mut [&mut [f32]],
                        coverage_row: &mut [f32],
                        confidence_row: &mut [f32]| {
            scratch.positions.fill(y, width, warp_transform, size);
            let positions = scratch.positions.positions();
            quality::write_row(positions, size, config.method, coverage_row, confidence_row);
            match &masked {
                None => {
                    for (channel, output_row) in channel_rows.iter_mut().enumerate() {
                        row::sample_row(
                            image.channel(channel),
                            positions,
                            config.method,
                            config.border_value,
                            output_row,
                        );
                    }
                }
                Some(masked) => masked.warp_row(
                    positions,
                    config,
                    &mut scratch.support,
                    channel_rows,
                    coverage_row,
                    confidence_row,
                ),
            }
        };

        let Self {
            pixels,
            coverage,
            confidence,
            rows,
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
    }
}

#[cfg(test)]
pub(super) mod internals {
    use imaginarium::Buffer2;
    use rayon::prelude::*;

    use crate::math::size2us::Size2us;
    use crate::registration::config::WarpParams;
    use crate::registration::resample::row;
    use crate::registration::resample::row_positions::RowPositions;
    use crate::registration::transform::WarpTransform;

    /// One plane warped on its own, without the quality maps — the shape the kernel tests and the
    /// plane benches compare.
    pub(crate) fn warp_plane(
        input: &Buffer2<f32>,
        output: &mut Buffer2<f32>,
        transform: &WarpTransform,
        params: WarpParams,
    ) {
        let size = Size2us::new(input.width(), input.height());
        debug_assert_eq!((output.width(), output.height()), (size.width, size.height));
        output
            .pixels_mut()
            .par_chunks_mut(size.width)
            .enumerate()
            .for_each_init(RowPositions::default, |positions, (y, output_row)| {
                positions.fill(y, size.width, transform, size);
                row::sample_row(
                    input,
                    positions.positions(),
                    params.method,
                    params.border_value,
                    output_row,
                );
            });
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
