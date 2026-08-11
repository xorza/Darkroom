//! Chunked combine engine for resident and memory-mapped stacking frames.

pub(crate) mod core;
mod loader;
pub(crate) mod sample;
pub(crate) mod validation;

use std::sync::OnceLock;

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::concurrency::JobScratchPool;
use crate::error::FrameDimensionMismatch;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::stacking::combine::cache::core::{
    CacheCore, ChunkContext, coverage_chunk_memory_layout, quality_plane_chunks,
    weighted_chunk_memory_layout,
};
use crate::stacking::combine::cache::sample::{CombineScratch, CombinedSample};
use crate::stacking::combine::cache::validation::{
    validate_image_samples, validate_sample_spans, validate_stored_geometry,
    validate_stored_samples, validate_warp_quality,
};
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::error::check_cancel;
use crate::stacking::combine::normalization::{FrameNorm, compute_frame_norms};
use crate::stacking::combine::pixel_coverage::PixelCoverage;
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::stacking::combine::stack::StackFrame;
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::spill::SpillDirectory;
use crate::stacking::frame_store::warp_quality::WarpQuality;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::stack_product::StackProduct;
use crate::stacking::stack_product::coverage::Coverage;
use crate::stacking::stack_product::quality_map::QualityMap;
use crate::stacking::stack_product::quality_planes::QualityPlanes;

/// Channel-shaped result of one combine pass. A plane is `None` when [`QualityPlanes`] did not
/// ask for it.
#[derive(Debug)]
pub(crate) struct CombineOutput {
    pub(super) pixels: LinearPixels,
    weight: Option<LinearPixels>,
    linear_variance: Option<LinearPixels>,
}

/// The output rows one combine row-task writes: the combined value, plus whichever ancillary
/// planes were requested. Bundling them keeps one gather loop instead of one per plane subset.
#[derive(Debug)]
struct QualityRows<'a> {
    value: &'a mut [f32],
    weight: Option<&'a mut [f32]>,
    linear_variance: Option<&'a mut [f32]>,
}

/// The frames feeding one combine, with their normalization parameters. Calibration masters and
/// registered light stacks share it; a calibration frame simply carries no warp quality planes.
#[derive(Debug)]
pub(crate) struct FrameCache {
    // Stored planes drop before the spill directory owner in `core`.
    pub(crate) frames: Vec<StoredFrame>,
    pub(crate) frame_norms: Option<Vec<FrameNorm>>,
    /// The normalization `frame_norms` was measured for. Kept so the combine can confirm the
    /// `StackConfig` it is handed asks for the normalization the cache was actually built with —
    /// the parameters are fixed at construction and never recomputed.
    pub(crate) normalization: Normalization,
    pub(crate) core: CacheCore,
}

#[derive(Debug)]
pub(crate) struct FrameCacheParams {
    pub(crate) spill_directory: Option<SpillDirectory>,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) metadata: ImageMetadata,
    pub(crate) config: CacheConfig,
    pub(crate) normalization: Normalization,
    pub(crate) progress: ProgressCallback,
    pub(crate) cancel: CancelToken,
}

impl FrameCache {
    /// Build a cache from frames already placed in the shared frame store.
    pub(crate) fn from_stored_frames(
        frames: Vec<StoredFrame>,
        params: FrameCacheParams,
    ) -> Result<Self, Error> {
        let FrameCacheParams {
            spill_directory,
            dimensions,
            metadata,
            config,
            normalization,
            progress,
            cancel,
        } = params;
        check_cancel(&cancel)?;
        // Before any geometry or contents: two frames divided by different spans are not the same
        // measurement, and every check below would pass on them.
        validate_sample_spans(&frames)?;
        for (index, frame) in frames.iter().enumerate() {
            // Geometry before contents: every read below and in the combine slices a plane to
            // `pixel_count`, so a short plane would panic out of a slice index rather than
            // reporting which frame was the wrong shape.
            validate_stored_geometry(frame, dimensions, index)?;
            validate_stored_samples(&frame.channels, dimensions.pixel_count(), index, &cancel)?;
            // Same guarantee `from_stack_frames` gives caller-supplied planes: each plane in range
            // and the two agreeing on where the frame has support, so the gate and the weight
            // multiplier below can't be handed a value that silently corrupts the combine.
            if let WarpQuality::Planes {
                coverage,
                confidence,
            } = &frame.quality
            {
                let pixel_count = dimensions.pixel_count();
                validate_warp_quality(
                    index,
                    coverage.chunk(0, pixel_count),
                    confidence.chunk(0, pixel_count),
                    &cancel,
                )?;
            }
        }
        let frame_norms = compute_frame_norms(&frames, dimensions, normalization, &cancel)?;
        Ok(Self {
            frames,
            frame_norms,
            normalization,
            core: CacheCore {
                spill_directory,
                dimensions,
                metadata,
                config,
                progress,
                cancel,
                chunk_memory: OnceLock::new(),
            },
        })
    }

    /// Build an in-memory warp-quality-aware cache from [`StackFrame`]s.
    pub(crate) fn from_stack_frames(
        frames: Vec<StackFrame>,
        config: &CacheConfig,
        normalization: Normalization,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        if frames.is_empty() {
            return Err(Error::NoFrames);
        }
        check_cancel(&cancel)?;
        let dimensions = frames[0].image.dimensions();
        let metadata = frames[0].image.metadata.clone();

        for (index, frame) in frames.iter().enumerate() {
            check_cancel(&cancel)?;
            if index > 0 {
                FrameDimensionMismatch::check(index, dimensions, frame.image.dimensions())?;
            }
            validate_image_samples(&frame.image, index, &cancel)?;
            // Geometry before contents, as in `from_stored_frames`: `pixels()` is read to the
            // plane's own length, so a wrong-shaped plane has to be named here rather than
            // reported as bad values.
            for (kind, plane) in frame.quality.present() {
                if (plane.width(), plane.height()) != (dimensions.width(), dimensions.height()) {
                    return Err(Error::WarpPlaneDimensionMismatch {
                        index,
                        plane: kind,
                        expected_width: dimensions.width(),
                        expected_height: dimensions.height(),
                        actual_width: plane.width(),
                        actual_height: plane.height(),
                    });
                }
            }
            if let WarpQuality::Planes {
                coverage,
                confidence,
            } = &frame.quality
            {
                validate_warp_quality(index, coverage.pixels(), confidence.pixels(), &cancel)?;
            }
        }
        check_cancel(&cancel)?;
        let stored = frames
            .into_iter()
            .map(|frame| StoredFrame::from_memory(frame.image, frame.quality, frame.source_stats))
            .collect::<Vec<_>>();
        validate_sample_spans(&stored)?;
        let frame_norms = compute_frame_norms(&stored, dimensions, normalization, &cancel)?;

        Ok(Self {
            frames: stored,
            frame_norms,
            normalization,
            core: CacheCore {
                spill_directory: None,
                dimensions,
                metadata,
                config: config.clone(),
                progress,
                cancel,
                chunk_memory: OnceLock::new(),
            },
        })
    }

    /// Assemble the combined image, geometric coverage, and per-channel survivor quality.
    pub(crate) fn finish_product(
        &self,
        combined: CombineOutput,
        planes: QualityPlanes,
        quantization_sigma: Option<f32>,
    ) -> StackProduct {
        let CombineOutput {
            pixels,
            weight: weight_pixels,
            linear_variance: linear_variance_pixels,
        } = combined;
        let dimensions = self.core.dimensions;
        let image = LinearImage {
            metadata: self.core.metadata.clone(),
            pixels,
        };
        let weight = weight_pixels.map(QualityMap::from_pixels);
        let linear_variance = linear_variance_pixels.map(QualityMap::from_pixels);
        let frame_count = self.frames.len();
        let width = dimensions.width();
        let height = dimensions.height();

        // No frame carries support, so every pixel is fully covered — and saying so costs one
        // number rather than an image-sized plane of `1.0`.
        if !planes.coverage || self.frames.iter().all(|frame| frame.quality.is_none()) {
            return StackProduct {
                image,
                coverage: planes.coverage.then(|| Coverage::Uniform {
                    value: 1.0,
                    size: dimensions.size(),
                }),
                weight,
                linear_variance,
                quantization_sigma,
            };
        }

        let mut coverage = Buffer2::new_default(width, height);
        let inv_frames = 1.0 / frame_count as f32;

        // Coverage planes share their frame's tier, so they may be mmap-backed: read them in the
        // same row-aligned chunks the combine uses, against the reading the combine sized against
        // — `CacheCore::chunk_available_memory` took it there and hands back the same figure here.
        let chunk_rows = self
            .core
            .chunk_available_memory()
            .map_or(height, |available_memory| {
                coverage_chunk_memory_layout(&self.frames, dimensions.channels(), planes)
                    .optimal_chunk_rows(dimensions.size(), available_memory)
            });

        let mut start_row = 0;
        while start_row < height {
            let end_row = (start_row + chunk_rows).min(height);
            let base = start_row * width;
            let span = (end_row - start_row) * width;

            let cov_chunks = quality_plane_chunks(
                &self.frames,
                |frame| frame.quality.coverage(),
                base,
                base + span,
            );

            let cov_out = &mut coverage.pixels_mut()[base..base + span];
            cov_out
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(row_in_chunk, cov_row)| {
                    let row_base = row_in_chunk * width;
                    for (px, output) in cov_row.iter_mut().enumerate() {
                        let local = row_base + px;
                        // The combine's own gate, so the fraction reported here is exactly the
                        // fraction of frames whose sample entered the statistics.
                        let count = cov_chunks
                            .iter()
                            .filter(|cov| {
                                cov.map_or(PixelCoverage::FULL, |map| {
                                    PixelCoverage::new(map[local])
                                })
                                .contributes()
                            })
                            .count();
                        *output = count as f32 * inv_frames;
                    }
                });

            start_row = end_row;
        }

        StackProduct {
            image,
            coverage: Some(Coverage::PerPixel(coverage)),
            weight,
            linear_variance,
            quantization_sigma,
        }
    }

    /// The combine: for each output pixel, gather the frames that cover it, hand them to
    /// `combine`, and write the reduced value plus whichever [`QualityPlanes`] were requested.
    ///
    /// [`PixelCoverage`] alone decides whether a frame is gathered at a pixel; its confidence then
    /// scales the weight the sample carries into the reduction, and is guaranteed positive wherever
    /// the frame was gathered. A frame carrying neither plane contributes everywhere at unit
    /// confidence, which is what lets calibration masters and registered light stacks share this
    /// loop. A pixel no frame supports gets `0`.
    pub(crate) fn process_chunked<Combine>(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        planes: QualityPlanes,
        combine: Combine,
    ) -> CombineOutput
    where
        Combine: Fn(&mut [f32], &[f32], &mut ScratchBuffers) -> CombinedSample + Sync,
    {
        if let Some(w) = weights {
            assert_eq!(
                w.len(),
                self.frames.len(),
                "Weight count must match frame count"
            );
        }
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.cancel.clone();
        let dimensions = self.core.dimensions;
        let memory = weighted_chunk_memory_layout(&self.frames, dimensions.channels(), planes);
        // Coverage sizing must reuse this pre-output snapshot or resident planes are charged twice.
        let mut output_weight = planes.weight.then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_linear_variance = planes
            .variance
            .then(|| LinearPixels::new_zeroed(dimensions));
        // One pool for the whole combine. `process_chunks` invokes the row loop below once per
        // chunk per channel, so the leases have to outlive any single `for_each_init`.
        let scratch_pool = JobScratchPool::<CombineScratch>::default();
        let pixels = self.core.process_chunks(
            &self.frames,
            |frame| &frame.channels,
            memory,
            self.core.chunk_available_memory(),
            |output_slice, ctx| {
                let ChunkContext {
                    frames,
                    width,
                    channel,
                    pixel_offset,
                } = ctx;
                let frame_count = frames.len();
                let chunk_pixels = output_slice.len();
                // Per-frame support and confidence slices; `None` means full support/unit confidence.
                let chunk_end = pixel_offset + chunk_pixels;
                let coverage = quality_plane_chunks(
                    &self.frames,
                    |frame| frame.quality.coverage(),
                    pixel_offset,
                    chunk_end,
                );
                let confidence = quality_plane_chunks(
                    &self.frames,
                    |frame| frame.quality.confidence(),
                    pixel_offset,
                    chunk_end,
                );
                // One row bundle per output row, so an unrequested plane simply has no slice
                // to write instead of needing its own copy of the gather loop below.
                let mut rows: Vec<QualityRows<'_>> = output_slice
                    .chunks_mut(width)
                    .map(|value| QualityRows {
                        value,
                        weight: None,
                        linear_variance: None,
                    })
                    .collect();
                if let Some(plane) = output_weight.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.weight = Some(chunk);
                    }
                }
                if let Some(plane) = output_linear_variance.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.linear_variance = Some(chunk);
                    }
                }
                rows.into_par_iter().enumerate().for_each_init(
                    || {
                        let mut scratch = scratch_pool.acquire();
                        scratch.resize(frame_count);
                        scratch
                    },
                    |scratch, (row_in_chunk, mut row)| {
                        // Cancelled: skip the row's work (output stays zero; the
                        // caller discards the partial result and reports Cancelled).
                        if cancel.is_cancelled() {
                            return;
                        }
                        // One deref, then disjoint field borrows — `combine` takes two of them
                        // at once, which `scratch.values` / `scratch.buffers` through the lease's
                        // `DerefMut` could not provide.
                        let CombineScratch {
                            values,
                            eff_weights,
                            buffers,
                        } = &mut **scratch;
                        let row_offset = row_in_chunk * width;
                        for pixel_in_row in 0..width {
                            let pixel_idx = row_offset + pixel_in_row;
                            let mut covered = 0usize;
                            for (frame_idx, chunk) in frames.iter().enumerate() {
                                let support = match coverage[frame_idx] {
                                    Some(map) => PixelCoverage::new(map[pixel_idx]),
                                    None => PixelCoverage::FULL,
                                };
                                let q = match confidence[frame_idx] {
                                    Some(map) => map[pixel_idx],
                                    None => 1.0,
                                };
                                if support.contributes() {
                                    let v = match frame_norms {
                                        Some(fnm) => {
                                            let cn = fnm[frame_idx].channels[channel];
                                            chunk[pixel_idx] * cn.gain + cn.offset
                                        }
                                        None => chunk[pixel_idx],
                                    };
                                    values[covered] = v;
                                    eff_weights[covered] =
                                        weights.map_or(1.0, |w| w[frame_idx]) * q;
                                    covered += 1;
                                }
                            }
                            let sample = if covered == 0 {
                                CombinedSample::default()
                            } else {
                                debug_assert!(
                                    values[..covered].iter().all(|v| v.is_finite()),
                                    "non-finite pixel value entered the combine",
                                );
                                combine(&mut values[..covered], &eff_weights[..covered], buffers)
                            };
                            row.value[pixel_in_row] = sample.value;
                            if let Some(weight) = row.weight.as_deref_mut() {
                                weight[pixel_in_row] = sample.weight;
                            }
                            if let Some(variance) = row.linear_variance.as_deref_mut() {
                                variance[pixel_in_row] = sample.linear_variance;
                            }
                        }
                    },
                );
            },
        );
        CombineOutput {
            pixels,
            weight: output_weight,
            linear_variance: output_linear_variance,
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::sync::OnceLock;

    use common::CancelToken;

    use crate::stacking::combine::cache::FrameCache;
    use crate::stacking::combine::cache::core::CacheCore;
    use crate::stacking::combine::cache_config::CacheConfig;
    use crate::stacking::combine::config::Normalization;
    use crate::stacking::combine::normalization::compute_frame_norms;
    use crate::stacking::frame_store::frame_stats::FrameStats;
    use crate::stacking::frame_store::warp_quality::WarpQuality;
    use crate::stacking::frame_store::{StackableImage, StoredFrame};
    use crate::stacking::progress::ProgressCallback;

    impl FrameCache {
        /// An in-memory cache over already-decoded frames — the shape `from_paths` builds, without
        /// the file round-trip. The frames carry no warp quality, so this is the plain-combine
        /// cache the calibration path uses.
        pub(crate) fn from_images<I: StackableImage>(
            images: Vec<I>,
            normalization: Normalization,
        ) -> Self {
            let dimensions = images[0].dimensions();
            let metadata = images[0].metadata().clone();
            let frames: Vec<StoredFrame> = images
                .into_iter()
                .map(|image| {
                    let source_stats = FrameStats::measure(&image);
                    StoredFrame::from_memory(image, WarpQuality::None, source_stats)
                })
                .collect();
            let core = CacheCore {
                spill_directory: None,
                dimensions,
                metadata,
                config: CacheConfig::default(),
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
                chunk_memory: OnceLock::new(),
            };
            let frame_norms = compute_frame_norms(&frames, dimensions, normalization, &core.cancel)
                .expect("frames without coverage have no failing normalization path");
            Self {
                frames,
                frame_norms,
                normalization,
                core,
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
