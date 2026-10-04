//! Pixel distribution and accumulation for drizzle reconstruction.

pub(crate) mod frame_source;
mod kernel_plan;
mod output_band;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use std::ops::Range;

use crate::concurrency::JobScratchPool;
use crate::drizzle::accumulator::frame_source::FrameSource;
use crate::drizzle::accumulator::kernel_plan::KernelPlan;
use crate::drizzle::accumulator::output_band::{OutputBand, RadialScratch};
use crate::drizzle::config::DrizzleConfig;
use crate::drizzle::drizzle_result::DrizzleResult;
use crate::drizzle::error::DrizzleError;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::frame_stats::FrameStats;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::registration::transform::WarpTransform;
use crate::run_report::RunReport;
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::quality_map::QualityMap;

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
    /// Non-negative per-frame quality weight.
    pub weight: f32,
    /// Optional non-negative per-pixel quality weights with the same dimensions as the image.
    pub pixel_weight_map: Option<Buffer2<f32>>,
}

impl<T> DrizzleFrame<T> {
    /// Create an equally weighted frame without a per-pixel weight map.
    pub const fn new(source: T, warp: WarpTransform) -> Self {
        Self {
            source,
            warp,
            weight: 1.0,
            pixel_weight_map: None,
        }
    }
}

/// Every output plane restricted to one contiguous span of the grid.
///
/// The shape both phases of a drizzle work in: a band of rows while frames are scattered in, a
/// chunk of pixels while the result is normalized out. Splitting every plane on the same boundaries
/// is what lets one traversal touch all of them, so the weight map is read once rather than once
/// per plane that divides by it.
#[derive(Debug)]
struct PlaneSpan<'a> {
    /// Weighted flux per channel.
    data: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    weight: &'a mut [f32],
    /// `Σwᵢ²·vᵢ` per channel; empty when the config declines the variance plane.
    variance: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    counts: Option<&'a mut [f32]>,
}

/// Drizzle accumulator for building the output image.
#[derive(Debug)]
pub struct DrizzleAccumulator {
    input_dims: ImageDimensions,
    output: Size2us,
    frames_added: usize,
    /// Accumulated weighted flux values (`Σ fluxᵢ·wᵢ`), one Buffer2 per channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated drizzle weight `Σ wᵢ` per output pixel. Channel-independent (the per-pixel
    /// `wᵢ` is purely geometric × frame weight), so a single map serves all channels.
    weight: Buffer2<f32>,
    /// `Σwᵢ²·vᵢ` per channel, `vᵢ` each input pixel's model variance at its own value: divided by
    /// `(Σwᵢ)²` it is the output's variance, which the correlation-suppressed image RMS
    /// understates.
    ///
    /// Empty when the config declines the variance plane, which is the one quality output with a
    /// cost beyond its own allocation: these are output-grid planes resident for the whole run, a
    /// noise measurement per frame, and more arithmetic at every deposit.
    variance: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// How many frames deposited any flux at each output pixel, when the config asks for coverage.
    frame_counts: Option<Buffer2<f32>>,
    /// Configuration.
    config: DrizzleConfig,
    /// The kernel's constants, resolved from `config` once for the run.
    plan: KernelPlan,
    /// Input pixels whose position the warp's SIP correction could not invert, over every frame.
    unconverged_points: usize,
    /// The coverage bitsets of every band, one bit per output pixel, cut per frame into each band's
    /// share. Empty unless the config asks for coverage.
    touched: Vec<u64>,
    /// The input rows each band scans, recomputed per frame.
    scans: Vec<Range<usize>>,
    radial: JobScratchPool<RadialScratch>,
    /// Band height forced by the band-invariance test, which needs to compare one band against many
    /// on the same input. Production always derives it from the thread count.
    #[cfg(test)]
    band_rows_override: Option<usize>,
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for the given input dimensions.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` is invalid.
    #[expect(
        clippy::cast_sign_loss,
        reason = "validate holds the scale positive, so each output side is non-negative"
    )]
    pub fn new(input_dims: ImageDimensions, config: DrizzleConfig) -> Result<Self, DrizzleError> {
        config.validate()?;
        let output = Size2us::new(
            (input_dims.width() as f32 * config.scale).ceil() as usize,
            (input_dims.height() as f32 * config.scale).ceil() as usize,
        );

        let mut data = ArrayVec::new();
        for _ in 0..input_dims.channels() {
            data.push(Buffer2::new_default(output.width, output.height));
        }

        Ok(Self {
            input_dims,
            output,
            frames_added: 0,
            data,
            weight: Buffer2::new_default(output.width, output.height),
            variance: if config.quality.variance {
                (0..input_dims.channels())
                    .map(|_| Buffer2::new_default(output.width, output.height))
                    .collect()
            } else {
                ArrayVec::new()
            },
            frame_counts: config
                .quality
                .coverage
                .then(|| Buffer2::new_default(output.width, output.height)),
            plan: KernelPlan::new(&config),
            config,
            unconverged_points: 0,
            touched: Vec::new(),
            scans: Vec::new(),
            radial: JobScratchPool::default(),
            #[cfg(test)]
            band_rows_override: None,
        })
    }

    /// Validate and add one coherent frame to the accumulator.
    ///
    /// # Errors
    ///
    /// Returns an error when image dimensions differ from the accumulator, or when frame or pixel
    /// weights are negative or non-finite. The accumulator is unchanged on error.
    pub fn add_frame(&mut self, frame: &DrizzleFrame<LinearImage>) -> Result<(), DrizzleError> {
        self.validate(frame)?;
        self.frames_added += 1;
        // A frame carrying no weight deposits nothing anywhere, and the coverage plane must not
        // claim it reached the pixels it would have touched — the same reading a zero *pixel*
        // weight gets in `FrameSource`.
        if frame.weight == 0.0 {
            return Ok(());
        }

        // Measured on the frame as it arrives: drizzle takes no statistics from a caller.
        let noise = if self.variance.is_empty() {
            ArrayVec::new()
        } else {
            let stats = FrameStats::measure(&frame.source);
            (0..frame.source.channels())
                .map(|channel| stats.ccd_noise(channel))
                .collect()
        };
        let source = FrameSource::new(
            &frame.source,
            &frame.warp,
            f64::from(self.config.scale),
            frame.weight,
            frame.pixel_weight_map.as_ref(),
            noise,
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
        let words_per_band = if self.frame_counts.is_some() {
            (width * band_rows).div_ceil(u64::BITS as usize)
        } else {
            0
        };
        self.touched.resize(words_per_band * self.scans.len(), 0);

        let Self {
            data,
            weight,
            variance,
            frame_counts,
            touched,
            scans,
            radial,
            ..
        } = &mut *self;
        let mut touched = touched.chunks_mut(words_per_band.max(1));
        let bands: Vec<OutputBand<'_>> = split_planes(
            data,
            weight,
            variance,
            frame_counts.as_mut(),
            width * band_rows,
        )
        .into_iter()
        .enumerate()
        .map(|(index, planes)| {
            let start = index * band_rows;
            let bits = if words_per_band == 0 {
                &mut [][..]
            } else {
                touched.next().expect("one bitset per band")
            };
            OutputBand::new(
                start..(start + band_rows).min(height),
                width,
                planes,
                bits,
                index,
                scans,
            )
        })
        .collect();
        let unconverged: usize = bands
            .into_par_iter()
            .map(|mut band| band.distribute(&source, plan, radial))
            .sum();
        self.unconverged_points += unconverged;
        Ok(())
    }

    /// Finalize the drizzle result: normalize flux by weight and emit whichever quality planes
    /// [`DrizzleConfig::quality`] asked for. The weight `Σwᵢ` is channel-independent, so one map
    /// normalizes every channel and seeds the quality outputs.
    ///
    /// The weight map is built whatever the request — the image is `Σfluxᵢwᵢ / Σwᵢ`, and
    /// `min_weight_fraction` gates fill against its maximum — so declining `weight` only declines
    /// handing it out, while declining `coverage` or `variance` skips an output-grid allocation
    /// each.
    ///
    /// The Lanczos kernel's negative lobes can leave a pixel below zero, and it stays there: on
    /// background-subtracted data a sample near zero is as likely negative as positive, and
    /// clamping would bias it upwards.
    pub fn finalize(mut self) -> DrizzleResult {
        let fill_value = self.config.fill_value;

        // The fill gate is a share of the deepest pixel's weight, so it needs that maximum. Floored
        // at the smallest positive float so "covered at all" and "covered enough" are one
        // comparison — a weight below that divides flux into an infinity rather than an image.
        let max_weight = self
            .weight
            .pixels()
            .par_iter()
            .copied()
            .reduce(|| 0.0f32, f32::max);
        let threshold = (self.config.min_weight_fraction * max_weight).max(f32::MIN_POSITIVE);
        // Coverage is the share of frames that reached each pixel — the same quantity the
        // statistical combine reports, so a reader need not know which producer built the product.
        // It is deliberately *not* `weight / max_weight`: that answers how deep a pixel is relative
        // to the best-covered one in this particular run, which is a different question and remains
        // available as `weight` divided by its own maximum.
        let inv_frames = if self.frames_added > 0 {
            1.0 / self.frames_added as f32
        } else {
            0.0
        };

        // One traversal for every plane at once. Each is an output-grid plane — `scale²` times the
        // input — so a pass per plane would read the weight map once per channel and again per
        // quality output, and the normalization is memory-bound. In place for the same reason:
        // collecting fresh planes would hold the accumulated and normalized grids at once.
        let span_len = self.output.width * Self::balanced_band_rows(self.output.height);
        split_planes(
            &mut self.data,
            &mut self.weight,
            &mut self.variance,
            self.frame_counts.as_mut(),
            span_len,
        )
        .into_par_iter()
        .for_each(|mut span| {
            for index in 0..span.weight.len() {
                let weight = span.weight[index];
                // A pixel below the gate holds the fill value, which no frame measured: it carries
                // no weight, no variance and no coverage, so no plane claims a measurement there.
                let covered = weight >= threshold;
                for plane in &mut span.data {
                    plane[index] = if covered {
                        plane[index] / weight
                    } else {
                        fill_value
                    };
                }
                for plane in &mut span.variance {
                    plane[index] = if covered {
                        plane[index] / (weight * weight)
                    } else {
                        0.0
                    };
                }
                if let Some(counts) = &mut span.counts {
                    counts[index] = if covered {
                        counts[index] * inv_frames
                    } else {
                        0.0
                    };
                }
                if !covered {
                    span.weight[index] = 0.0;
                }
            }
        });

        let dimensions = ImageDimensions::new(self.output, self.data.len());
        let image = LinearImage::from_planar_channels(
            dimensions,
            self.data.into_iter().map(Buffer2::into_vec),
        );
        let product = StackProduct {
            image,
            coverage: self.frame_counts.map(Coverage::PerPixel),
            weight: self
                .config
                .quality
                .weight
                .then_some(QualityMap::Shared(self.weight)),
            variance: (!self.variance.is_empty()).then(|| QualityMap::from_planes(self.variance)),
            // Drizzle takes demosaiced frames, which carry no mosaic.
            cfa_type: None,
            // Drizzle leaves out every flagged pixel it cannot use, with no survivor floor to keep
            // one for: there is nothing per pixel to report.
            report: RunReport::default(),
        };
        DrizzleResult {
            product,
            unconverged_points: self.unconverged_points,
        }
    }

    fn validate(&self, frame: &DrizzleFrame<LinearImage>) -> Result<(), DrizzleError> {
        let index = self.frames_added;
        FrameDimensionMismatch::check(index, self.input_dims, frame.source.dimensions())?;
        if !frame.weight.is_finite() || frame.weight < 0.0 {
            return Err(DrizzleError::InvalidFrameWeight {
                index,
                value: frame.weight,
            });
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
        // `find_first` rather than `find_any`: the reported pixel is part of the error, so the
        // frame that fails has to name the same one every run.
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

    /// How many output rows one span covers.
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

/// Every output plane cut into `span_len`-long spans, aligned across planes.
///
/// Every plane is output-sized, so one span length splits all of them the same way and the spans
/// are disjoint by construction.
fn split_planes<'a>(
    data: &'a mut ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    weight: &'a mut Buffer2<f32>,
    variance: &'a mut ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    counts: Option<&'a mut Buffer2<f32>>,
    span_len: usize,
) -> Vec<PlaneSpan<'a>> {
    debug_assert!(span_len > 0);
    let mut data: ArrayVec<_, MAX_CHANNELS> = data
        .iter_mut()
        .map(|plane| plane.pixels_mut().chunks_mut(span_len))
        .collect();
    let mut variance: ArrayVec<_, MAX_CHANNELS> = variance
        .iter_mut()
        .map(|plane| plane.pixels_mut().chunks_mut(span_len))
        .collect();
    let mut counts = counts.map(|buffer| buffer.pixels_mut().chunks_mut(span_len));

    // The weight plane drives the split: it is the one plane that is always present, and every
    // other yields chunks in lockstep with it.
    weight
        .pixels_mut()
        .chunks_mut(span_len)
        .map(|weight| PlaneSpan {
            data: data
                .iter_mut()
                .map(|chunks| chunks.next().expect("one chunk per span per channel"))
                .collect(),
            weight,
            variance: variance
                .iter_mut()
                .map(|chunks| {
                    chunks
                        .next()
                        .expect("one variance chunk per span per channel")
                })
                .collect(),
            counts: counts
                .as_mut()
                .map(|chunks| chunks.next().expect("one coverage chunk per span")),
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::drizzle::accumulator::*;
    use crate::registration::transform::Transform;

    impl DrizzleAccumulator {
        /// Add `image` as an otherwise default frame, panicking on the mismatches a fixture must
        /// not have. `transform` maps input pixels onto the reference — the direction fixtures are
        /// written in — so the frame carries its inverse, the warp registration would report.
        pub(crate) fn add_image(
            &mut self,
            image: LinearImage,
            transform: &Transform,
            weight: f32,
            pixel_weights: Option<&Buffer2<f32>>,
        ) {
            self.add_frame(&DrizzleFrame {
                source: image,
                warp: WarpTransform::new(transform.inverse()),
                weight,
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
            self.add_image(image, transform, 1.0, None);
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

        /// `Σw` per output pixel as deposited, before `finalize` gates it.
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
    }
}
