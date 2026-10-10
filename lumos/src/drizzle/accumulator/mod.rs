//! Pixel distribution and accumulation for drizzle reconstruction.

pub(crate) mod frame_source;
mod kernel_plan;
mod output_band;

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;
use rayon::prelude::*;

use std::slice::ChunksMut;

use crate::concurrency::job_scratch_pool::JobScratchPool;
use crate::drizzle::accumulator::frame_source::{BandScan, FrameSource};
use crate::drizzle::accumulator::kernel_plan::KernelPlan;
use crate::drizzle::accumulator::output_band::{CornerLattice, OutputBand, RadialScratch};
use crate::drizzle::config::DrizzleConfig;
use crate::drizzle::deposit::Deposit;
use crate::drizzle::error::DrizzleError;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::frame_quality::DropPlanes;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::size2us::Size2us;
use crate::registration::transform::WarpTransform;

/// A frame is grayscale or RGB — the two shapes `LinearImage` has — or a mosaic of at most three
/// colours, so it never carries more planes, and the output never more channels.
const MAX_CHANNELS: usize = 3;
/// Output bands per rayon worker — enough for work-stealing to even out bands that different
/// amounts of the input reach.
const BANDS_PER_WORKER: usize = 4;

/// One drizzle input and all metadata that must remain aligned with it.
#[derive(Debug, Clone)]
pub struct DrizzleFrame<T> {
    /// Image or path to load: a demosaiced or mono frame, or a calibrated mosaic whose photosites
    /// each reach the channel of their colour alone.
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
    /// `Σw` and `Σw²`: one plane every channel shares, or one per colour of a mosaic.
    weight: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    weight_sq: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    flags: Option<&'a mut [u8]>,
}

/// The drops of the frames added so far, summed on the output grid: `Σw·x` per channel, `Σw` and
/// `Σw²`, and the flags the drops carried.
#[derive(Debug)]
pub(crate) struct DrizzleAccumulator {
    input_dims: ImageDimensions,
    deposit: Deposit,
    output: Size2us,
    /// The run's index of the next frame added, which an error names.
    next_frame: usize,
    /// Accumulated weighted flux values (`Σ fluxᵢ·wᵢ`), one Buffer2 per output channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated drizzle weight `Σ wᵢ` per output pixel. The per-pixel `wᵢ` is purely geometric
    /// × pixel weight, so one plane serves every channel a drop reaches; a mosaic's photosites
    /// reach one channel each, so it keeps one per colour.
    weight: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// `Σ wᵢ²` per output pixel, in the planes of `weight`, whose ratio to `(Σwᵢ)²` is the drops'
    /// Kish size.
    weight_sq: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// The [`QualityFlags::RESAMPLE_CARRIED`] flags each output pixel's drops carried; allocated
    /// when a frame carrying any arrives.
    flags: Option<Buffer2<u8>>,
    config: DrizzleConfig,
    /// The kernel's constants, resolved from `config` once for the run.
    plan: KernelPlan,
    /// Input pixels whose position the warp's SIP correction could not invert, over every frame.
    unconverged_points: usize,
    /// The input pixels each band scans, recomputed per frame.
    scans: Vec<BandScan>,
    radial: JobScratchPool<RadialScratch>,
    lattice: JobScratchPool<CornerLattice>,
    /// Band height forced by the band-invariance test, which needs to compare one band against many
    /// on the same input. Production always derives it from the thread count.
    #[cfg(test)]
    band_rows_override: Option<usize>,
}

/// One frame drizzled on its own, as the combine reads it: where its drops landed, each channel
/// their weighted mean `Σw·x/Σw`, with their summed weight `Σw` and their Kish size `(Σw)²/Σw²` —
/// one pair every channel shares, or one per colour of a mosaic — and the flags they carried.
/// Where no drop landed in a channel, or the drops' weights sum to none, its pixels, weight and
/// size are 0, and nowhere else.
#[derive(Debug)]
pub(crate) struct DrizzledPlanes {
    pub(crate) pixels: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    pub(crate) drops: ArrayVec<DropPlanes<Buffer2<f32>>, MAX_CHANNELS>,
    pub(crate) flags: Option<PixelFlags>,
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for frames of `input_dims` whose samples reach the output
    /// as `deposit` says, whose first frame is the run's frame `first_frame`.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` is invalid.
    pub(crate) fn new(
        input_dims: ImageDimensions,
        deposit: Deposit,
        config: DrizzleConfig,
        first_frame: usize,
    ) -> Result<Self, DrizzleError> {
        config.validate()?;
        let output = config.output_size(input_dims.size());
        let planes = |count: usize| -> ArrayVec<_, MAX_CHANNELS> {
            (0..count)
                .map(|_| Buffer2::new_default(output.width, output.height))
                .collect()
        };
        Ok(Self {
            input_dims,
            deposit,
            output,
            next_frame: first_frame,
            data: planes(deposit.output_channels()),
            weight: planes(deposit.weight_planes()),
            weight_sq: planes(deposit.weight_planes()),
            flags: None,
            plan: KernelPlan::new(&config),
            config,
            unconverged_points: 0,
            scans: Vec::new(),
            radial: JobScratchPool::default(),
            lattice: JobScratchPool::default(),
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
    /// Returns an error when image dimensions or the mosaic pattern differ from the accumulator,
    /// when a sample is not finite, or when pixel weights are negative or non-finite. The
    /// accumulator is unchanged on error.
    pub(crate) fn add_frame(
        &mut self,
        frame: &DrizzleFrame<impl StackableImage>,
    ) -> Result<(), DrizzleError> {
        self.validate(frame)?;
        self.next_frame += 1;
        if self.flags.is_none()
            && frame
                .source
                .flags()
                .is_some_and(|flags| flags.contains(QualityFlags::RESAMPLE_CARRIED))
        {
            self.flags = Some(Buffer2::new_default(self.output.width, self.output.height));
        }
        let source = FrameSource::new(
            &frame.source,
            self.deposit,
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
                source.band_scan(
                    &(start..(start + band_rows).min(height)),
                    width,
                    reach.output,
                    reach.input,
                )
            }));

        let Self {
            data,
            weight,
            weight_sq,
            flags,
            scans,
            radial,
            lattice,
            unconverged_points,
            ..
        } = &mut *self;
        let span_len = width * band_rows;
        let mut data = band_chunks(data, span_len);
        let mut weight = band_chunks(weight, span_len);
        let mut weight_sq = band_chunks(weight_sq, span_len);
        let mut flags = flags
            .as_mut()
            .map(|flags| flags.pixels_mut().chunks_mut(span_len));
        let bands: Vec<OutputBand<'_>> = (0..height.div_ceil(band_rows))
            .map(|index| {
                let start = index * band_rows;
                let planes = PlaneSpan {
                    data: next_band(&mut data),
                    weight: next_band(&mut weight),
                    weight_sq: next_band(&mut weight_sq),
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
            .map(|mut band| band.distribute(&source, plan, radial, lattice))
            .sum::<usize>();
        Ok(())
    }

    /// The frames added, as one drizzled frame: see [`DrizzledPlanes`].
    pub(crate) fn into_planes(self) -> DrizzledPlanes {
        let Self {
            deposit,
            mut data,
            weight,
            weight_sq,
            flags,
            ..
        } = self;
        let drops: ArrayVec<_, MAX_CHANNELS> = weight
            .into_iter()
            .zip(weight_sq)
            .map(|(mut weight, mut confidence)| {
                // In place, `Σw²` becoming the Kish size. A pixel whose drops sum to no positive
                // weight — a Lanczos lobe's — or whose size underflows holds nothing the combine
                // could divide its noise by, so it is one no drop reached: the weight and the size
                // are 0 together, which is the pairing `FrameQuality` documents.
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
                DropPlanes { weight, confidence }
            })
            .collect();
        for (channel, plane) in data.iter_mut().enumerate() {
            let weight = &drops[deposit.weight_plane(channel)].weight;
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
            drops,
            flags: flags.and_then(PixelFlags::from_buffer),
        }
    }

    fn validate(&self, frame: &DrizzleFrame<impl StackableImage>) -> Result<(), DrizzleError> {
        let index = self.next_frame;
        FrameDimensionMismatch::check(index, self.input_dims, frame.source.dimensions())?;
        let deposit = Deposit::of(&frame.source);
        if deposit != self.deposit {
            return Err(DrizzleError::PatternMismatch {
                index,
                expected: self.deposit.mosaic(),
                actual: deposit.mosaic(),
            });
        }
        // `find_first` rather than `find_any`: the reported sample is part of the error, so the
        // frame that fails has to name the same one every run.
        for channel in 0..self.input_dims.channels() {
            if let Some((pixel, &value)) = frame
                .source
                .channel(channel)
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
    /// margin — over-scanning at a band boundary is nearly free (see `FrameSource::band_scan`), so
    /// there is nothing to trade off against balance.
    fn balanced_band_rows(output_height: usize) -> usize {
        let target = rayon::current_num_threads() * BANDS_PER_WORKER;
        output_height.div_ceil(target.max(1)).max(1)
    }
}

/// Each of `planes` cut into the spans of `len` samples the bands own, in band order.
fn band_chunks(
    planes: &mut [Buffer2<f32>],
    len: usize,
) -> ArrayVec<ChunksMut<'_, f32>, MAX_CHANNELS> {
    planes
        .iter_mut()
        .map(|plane| plane.pixels_mut().chunks_mut(len))
        .collect()
}

/// The next band's span of each plane [`band_chunks`] cut.
fn next_band<'a>(
    chunks: &mut ArrayVec<ChunksMut<'a, f32>, MAX_CHANNELS>,
) -> ArrayVec<&'a mut [f32], MAX_CHANNELS> {
    chunks
        .iter_mut()
        .map(|chunks| chunks.next().expect("one chunk per band per plane"))
        .collect()
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::drizzle::accumulator::*;
    use crate::io::image::linear::LinearImage;
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
            image: impl StackableImage,
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

        /// [`DrizzleAccumulator::add_frame`] with the output band height pinned, so a test can
        /// compare band counts.
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

        /// `Σw` per output pixel as deposited, in the plane every channel shares.
        ///
        /// # Panics
        /// For a mosaic's accumulator, whose weights are per colour.
        pub(crate) fn accumulated_weights(&self) -> &Buffer2<f32> {
            assert_eq!(self.weight.len(), 1, "the weights are per colour");
            &self.weight[0]
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
        /// pixel of its weight plane, beside its weight `Σw`, gated alike. The single-pass
        /// drizzle, which the combine of frames drizzled one at a time is held to.
        pub(crate) fn finalize(self) -> StackProduct {
            let fill_value = self.config.fill_value;
            let gate = self.config.min_weight_fraction;
            let deposit = self.deposit;
            let mut data = self.data;
            let mut weight = self.weight;
            let thresholds: ArrayVec<f32, MAX_CHANNELS> = weight
                .iter()
                .map(|weight| {
                    let deepest = weight.pixels().iter().copied().fold(0.0f32, f32::max);
                    (gate * deepest).max(f32::MIN_POSITIVE)
                })
                .collect();
            for (channel, plane) in data.iter_mut().enumerate() {
                let weights = deposit.weight_plane(channel);
                for (value, &weight) in plane.pixels_mut().iter_mut().zip(weight[weights].pixels())
                {
                    *value = if weight >= thresholds[weights] {
                        *value / weight
                    } else {
                        fill_value
                    };
                }
            }
            for (weight, &threshold) in weight.iter_mut().zip(&thresholds) {
                for weight in weight.pixels_mut() {
                    if *weight < threshold {
                        *weight = 0.0;
                    }
                }
            }
            let dimensions = ImageDimensions::new(self.output, data.len());
            let weight = match deposit {
                Deposit::Mosaic(_) => QualityMap::PerChannel(
                    weight
                        .into_inner()
                        .expect("a mosaic's three colours each keep a weight plane"),
                ),
                Deposit::Channels(_) => {
                    QualityMap::Shared(weight.into_iter().next().expect("one shared plane"))
                }
            };
            StackProduct {
                image: LinearImage::from_planar_channels(
                    dimensions,
                    data.into_iter().map(Buffer2::into_vec),
                ),
                coverage: None,
                weight: Some(weight),
                inverse_variance: None,
                dispersion: None,
                cfa_type: None,
                report: RunReport::default(),
            }
        }
    }
}
