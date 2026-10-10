//! Pixel distribution and accumulation for drizzle reconstruction.

pub(crate) mod frame_source;
mod kernel_plan;
mod output_band;

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;
use rayon::prelude::*;

use std::ops::Range;

use crate::concurrency::job_scratch_pool::JobScratchPool;
use crate::drizzle::accumulator::frame_source::FrameSource;
use crate::drizzle::accumulator::kernel_plan::KernelPlan;
use crate::drizzle::accumulator::output_band::{OutputBand, RadialScratch};
use crate::drizzle::config::DrizzleConfig;
use crate::drizzle::error::DrizzleError;
use crate::error::FrameDimensionMismatch;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::size2us::Size2us;
use crate::registration::transform::WarpTransform;

/// A frame is grayscale or RGB — the two shapes `LinearImage` has — so it never carries more
/// planes.
const MAX_CHANNELS: usize = 3;
/// Output bands per rayon worker — enough for work-stealing to even out bands that different
/// amounts of the input reach.
const BANDS_PER_WORKER: usize = 4;

/// One drizzle input and all metadata that must remain aligned with it.
#[derive(Debug, Clone)]
pub struct DrizzleFrame<T> {
    /// Image or path to load.
    pub source: T,
    /// The registration's warp, from the common reference grid to this input — what
    /// [`RegistrationResult::warp_transform`](crate::RegistrationResult::warp_transform) returns.
    /// Drizzle maps input pixels through its inverse, SIP included.
    pub warp: WarpTransform,
    /// Optional non-negative per-pixel quality weights with the same dimensions as the image.
    pub pixel_weight_map: Option<Buffer2<f32>>,
}

impl<T> DrizzleFrame<T> {
    /// A frame without a per-pixel weight map.
    pub const fn new(source: T, warp: WarpTransform) -> Self {
        Self {
            source,
            warp,
            pixel_weight_map: None,
        }
    }
}

/// Every output plane restricted to one contiguous span of the grid: a band of rows while a frame
/// is scattered in.
#[derive(Debug)]
struct PlaneSpan<'a> {
    /// Weighted flux per channel.
    data: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    weight: &'a mut [f32],
    weight_sq: &'a mut [f32],
    flags: Option<&'a mut [u8]>,
}

/// The drops of the frames added so far, summed on the output grid: `Σw·x` per channel, `Σw` and
/// `Σw²`, and the flags the drops carried.
#[derive(Debug)]
pub(crate) struct DrizzleAccumulator {
    input_dims: ImageDimensions,
    output: Size2us,
    /// The run's index of the next frame added, which an error names.
    next_frame: usize,
    /// Accumulated weighted flux values (`Σ fluxᵢ·wᵢ`), one Buffer2 per channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated drizzle weight `Σ wᵢ` per output pixel. Channel-independent (the per-pixel
    /// `wᵢ` is purely geometric × pixel weight), so a single map serves all channels.
    weight: Buffer2<f32>,
    /// `Σ wᵢ²` per output pixel, whose ratio to `(Σwᵢ)²` is the drops' Kish size.
    weight_sq: Buffer2<f32>,
    /// The [`QualityFlags::RESAMPLE_CARRIED`] flags each output pixel's drops carried; allocated
    /// when a frame carrying any arrives.
    flags: Option<Buffer2<u8>>,
    config: DrizzleConfig,
    /// The kernel's constants, resolved from `config` once for the run.
    plan: KernelPlan,
    /// Input pixels whose position the warp's SIP correction could not invert, over every frame.
    unconverged_points: usize,
    /// The input rows each band scans, recomputed per frame.
    scans: Vec<Range<usize>>,
    radial: JobScratchPool<RadialScratch>,
    /// Band height forced by the band-invariance test, which needs to compare one band against many
    /// on the same input. Production always derives it from the thread count.
    #[cfg(test)]
    band_rows_override: Option<usize>,
}

/// One frame drizzled on its own, as the combine reads it: where its drops landed, each channel
/// their weighted mean `Σw·x/Σw`, with their summed weight `Σw` and their Kish size `(Σw)²/Σw²`,
/// and the flags they carried. Where no drop landed, or the drops' weights sum to none, the
/// pixels, the weight and the size are 0, and nowhere else.
#[derive(Debug)]
pub(crate) struct DrizzledPlanes {
    pub(crate) pixels: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    pub(crate) weight: Buffer2<f32>,
    pub(crate) confidence: Buffer2<f32>,
    pub(crate) flags: Option<PixelFlags>,
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for the given input dimensions, whose first frame is the
    /// run's frame `first_frame`.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` is invalid.
    pub(crate) fn new(
        input_dims: ImageDimensions,
        config: DrizzleConfig,
        first_frame: usize,
    ) -> Result<Self, DrizzleError> {
        config.validate()?;
        let output = config.output_size(input_dims.size());
        Ok(Self {
            input_dims,
            output,
            next_frame: first_frame,
            data: (0..input_dims.channels())
                .map(|_| Buffer2::new_default(output.width, output.height))
                .collect(),
            weight: Buffer2::new_default(output.width, output.height),
            weight_sq: Buffer2::new_default(output.width, output.height),
            flags: None,
            plan: KernelPlan::new(&config),
            config,
            unconverged_points: 0,
            scans: Vec::new(),
            radial: JobScratchPool::default(),
            #[cfg(test)]
            band_rows_override: None,
        })
    }

    /// Input pixels, over every frame added, whose position a warp's SIP correction could not
    /// invert.
    pub(crate) const fn unconverged_points(&self) -> usize {
        self.unconverged_points
    }

    /// The reference point output pixel `output` lies at: the inverse of the output grid, reference
    /// pixel `p` landing at `s·p + (s − 1)/2`.
    pub(crate) fn reference_point(&self, output: DVec2) -> DVec2 {
        let scale = f64::from(self.config.scale);
        (output - (scale - 1.0) / 2.0) / scale
    }

    /// Validate and add one coherent frame to the accumulator.
    ///
    /// # Errors
    ///
    /// Returns an error when image dimensions differ from the accumulator, when a sample is not
    /// finite, or when pixel weights are negative or non-finite. The accumulator is unchanged on
    /// error.
    pub(crate) fn add_frame(
        &mut self,
        frame: &DrizzleFrame<LinearImage>,
    ) -> Result<(), DrizzleError> {
        self.validate(frame)?;
        self.next_frame += 1;
        if self.flags.is_none()
            && frame
                .source
                .flags
                .as_ref()
                .is_some_and(|flags| flags.contains(QualityFlags::RESAMPLE_CARRIED))
        {
            self.flags = Some(Buffer2::new_default(self.output.width, self.output.height));
        }
        let source = FrameSource::new(
            &frame.source,
            &frame.warp,
            f64::from(self.config.scale),
            frame.pixel_weight_map.as_ref(),
        );
        let plan = self.plan;
        let reach = plan.reach();

        let Size2us { width, height } = self.output;
        let band_rows = self.band_rows();
        self.scans.clear();
        self.scans
            .extend((0..height).step_by(band_rows).map(|start| {
                source.input_rows(
                    &(start..(start + band_rows).min(height)),
                    width,
                    reach.output_rows,
                    reach.input_rows,
                )
            }));

        let Self {
            data,
            weight,
            weight_sq,
            flags,
            scans,
            radial,
            unconverged_points,
            ..
        } = &mut *self;
        let span_len = width * band_rows;
        let mut data: ArrayVec<_, MAX_CHANNELS> = data
            .iter_mut()
            .map(|plane| plane.pixels_mut().chunks_mut(span_len))
            .collect();
        let mut weight_sq = weight_sq.pixels_mut().chunks_mut(span_len);
        let mut flags = flags
            .as_mut()
            .map(|flags| flags.pixels_mut().chunks_mut(span_len));
        let bands: Vec<OutputBand<'_>> = weight
            .pixels_mut()
            .chunks_mut(span_len)
            .enumerate()
            .map(|(index, weight)| {
                let start = index * band_rows;
                let planes = PlaneSpan {
                    data: data
                        .iter_mut()
                        .map(|chunks| chunks.next().expect("one chunk per band per channel"))
                        .collect(),
                    weight,
                    weight_sq: weight_sq.next().expect("one chunk per band"),
                    flags: flags
                        .as_mut()
                        .map(|chunks| chunks.next().expect("one chunk per band")),
                };
                OutputBand::new(
                    start..(start + band_rows).min(height),
                    width,
                    planes,
                    index,
                    scans,
                )
            })
            .collect();
        *unconverged_points += bands
            .into_par_iter()
            .map(|mut band| band.distribute(&source, plan, radial))
            .sum::<usize>();
        Ok(())
    }

    /// The frames added, as one drizzled frame: see [`DrizzledPlanes`].
    pub(crate) fn into_planes(self) -> DrizzledPlanes {
        let Self {
            mut data,
            mut weight,
            weight_sq: mut confidence,
            flags,
            ..
        } = self;
        // In place, `Σw²` becoming the Kish size. A pixel whose drops sum to no positive weight —
        // a Lanczos lobe's — or whose size underflows holds nothing the combine could divide its
        // noise by, so it is one no drop reached: the weight and the size are 0 together, which is
        // the pairing `FrameQuality` documents.
        weight
            .pixels_mut()
            .par_iter_mut()
            .zip(confidence.pixels_mut().par_iter_mut())
            .for_each(|(weight, confidence)| {
                *confidence = if *weight > 0.0 && *confidence > 0.0 {
                    (f64::from(*weight).powi(2) / f64::from(*confidence)) as f32
                } else {
                    0.0
                };
                if *confidence == 0.0 {
                    *weight = 0.0;
                }
            });
        for plane in &mut data {
            plane
                .pixels_mut()
                .par_iter_mut()
                .zip(weight.pixels().par_iter())
                .for_each(|(value, &weight)| {
                    *value = if weight > 0.0 { *value / weight } else { 0.0 };
                });
        }
        DrizzledPlanes {
            pixels: data,
            weight,
            confidence,
            flags: flags.and_then(PixelFlags::from_buffer),
        }
    }

    fn validate(&self, frame: &DrizzleFrame<LinearImage>) -> Result<(), DrizzleError> {
        let index = self.next_frame;
        FrameDimensionMismatch::check(index, self.input_dims, frame.source.dimensions())?;
        // `find_first` rather than `find_any`: the reported sample is part of the error, so the
        // frame that fails has to name the same one every run.
        for channel in 0..frame.source.channels() {
            if let Some((pixel, &value)) = frame
                .source
                .channel(channel)
                .pixels()
                .par_iter()
                .enumerate()
                .find_first(|(_, value)| !value.is_finite())
            {
                return Err(DrizzleError::NonFiniteSample {
                    index,
                    channel,
                    pixel,
                    value,
                });
            }
        }

        let Some(pixel_weights) = &frame.pixel_weight_map else {
            return Ok(());
        };
        if (pixel_weights.width(), pixel_weights.height())
            != (self.input_dims.width(), self.input_dims.height())
        {
            return Err(DrizzleError::PixelWeightDimensionMismatch {
                index,
                expected_width: self.input_dims.width(),
                expected_height: self.input_dims.height(),
                actual_width: pixel_weights.width(),
                actual_height: pixel_weights.height(),
            });
        }
        if let Some((pixel_index, &value)) = pixel_weights
            .pixels()
            .par_iter()
            .enumerate()
            .find_first(|(_, value)| !value.is_finite() || **value < 0.0)
        {
            return Err(DrizzleError::InvalidPixelWeight {
                frame_index: index,
                pixel_index,
                value,
            });
        }
        Ok(())
    }

    fn band_rows(&self) -> usize {
        #[cfg(test)]
        if let Some(rows) = self.band_rows_override {
            return rows;
        }
        Self::balanced_band_rows(self.output.height)
    }

    /// How many output rows one band covers.
    ///
    /// Several per worker so rayon can steal: a band's cost varies with how much of the input
    /// actually reaches it, which is not uniform once the transform rotates. Not tuned against the
    /// margin — over-scanning at a band boundary is nearly free (see `FrameSource::input_rows`), so
    /// there is nothing to trade off against balance.
    fn balanced_band_rows(output_height: usize) -> usize {
        let target = rayon::current_num_threads() * BANDS_PER_WORKER;
        output_height.div_ceil(target.max(1)).max(1)
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::drizzle::accumulator::*;
    use crate::registration::transform::Transform;
    use crate::run_report::RunReport;
    use crate::stack_product::StackProduct;
    use crate::stack_product::quality_map::QualityMap;

    impl DrizzleAccumulator {
        /// Add `image` as an otherwise default frame, panicking on the mismatches a fixture must
        /// not have. `transform` maps input pixels onto the reference — the direction fixtures are
        /// written in — so the frame carries its inverse, the warp registration would report.
        pub(crate) fn add_image(
            &mut self,
            image: LinearImage,
            transform: &Transform,
            pixel_weights: Option<&Buffer2<f32>>,
        ) {
            self.add_frame(&DrizzleFrame {
                source: image,
                warp: WarpTransform::new(transform.inverse()),
                pixel_weight_map: pixel_weights.cloned(),
            })
            .expect("test frame must be coherent with the accumulator");
        }

        /// [`DrizzleAccumulator::add_image`] with the output band height pinned, so a test can
        /// compare band counts.
        pub(crate) fn add_image_with_band_rows(
            &mut self,
            image: LinearImage,
            transform: &Transform,
            band_rows: usize,
        ) {
            self.band_rows_override = Some(band_rows);
            self.add_image(image, transform, None);
            self.band_rows_override = None;
        }

        /// [`DrizzleAccumulator::add_frame`] with the output band height pinned.
        pub(crate) fn add_frame_with_band_rows(
            &mut self,
            frame: &DrizzleFrame<LinearImage>,
            band_rows: usize,
        ) {
            self.band_rows_override = Some(band_rows);
            self.add_frame(frame)
                .expect("test frame must be coherent with the accumulator");
            self.band_rows_override = None;
        }

        /// `Σw` per output pixel as deposited.
        pub(crate) const fn accumulated_weights(&self) -> &Buffer2<f32> {
            &self.weight
        }

        /// `Σ flux·w` over channel `channel`'s accumulated plane, summed in f64.
        pub(crate) fn accumulated_flux_sum(&self, channel: usize) -> f64 {
            self.data[channel]
                .pixels()
                .iter()
                .map(|&value| f64::from(value))
                .sum()
        }

        /// Every frame added, drizzled into one grid at once: each pixel `Σw·x/Σw`, held at the
        /// fill value below the gate the config's `min_weight_fraction` sets against the deepest
        /// pixel, beside its weight `Σw`, gated alike. The single-pass drizzle, which the combine of
        /// frames drizzled one at a time is held to.
        pub(crate) fn finalize(self) -> StackProduct {
            let fill_value = self.config.fill_value;
            let max_weight = self.weight.pixels().iter().copied().fold(0.0f32, f32::max);
            let threshold = (self.config.min_weight_fraction * max_weight).max(f32::MIN_POSITIVE);
            let mut data = self.data;
            let mut weight = self.weight;
            for index in 0..weight.pixels().len() {
                let covered = weight[index] >= threshold;
                for plane in &mut data {
                    plane[index] = if covered {
                        plane[index] / weight[index]
                    } else {
                        fill_value
                    };
                }
                if !covered {
                    weight[index] = 0.0;
                }
            }
            let dimensions = ImageDimensions::new(self.output, data.len());
            StackProduct {
                image: LinearImage::from_planar_channels(
                    dimensions,
                    data.into_iter().map(Buffer2::into_vec),
                ),
                coverage: None,
                weight: Some(QualityMap::Shared(weight)),
                inverse_variance: None,
                dispersion: None,
                cfa_type: None,
                report: RunReport::default(),
            }
        }
    }
}
