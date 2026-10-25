//! Pixel distribution and accumulation for drizzle reconstruction.

mod band_scan;
mod drop_quad;
pub(crate) mod frame_source;
mod input_map;
pub(crate) mod kernel_plan;
mod output_band;

use std::slice::ChunksMut;

use arrayvec::ArrayVec;
use common::{Buffer2, par};
use imaginarium::PlanarDesc;
use rayon::prelude::*;

use crate::combine::error::StackError;
use crate::drizzle::accumulator::band_scan::BandScan;
use crate::drizzle::accumulator::frame_source::FrameSource;
use crate::drizzle::accumulator::kernel_plan::KernelPlan;
use crate::drizzle::accumulator::output_band::{CornerLattice, OutputBand, RadialScratch};
use crate::drizzle::deposit::Deposit;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::error::DrizzleError;
use crate::drizzle::output_grid::OutputGrid;
use crate::frame::frame_plane::FramePlane;
use crate::frame::frame_quality::DropPlanes;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::stackable_image::StackableImage;
use crate::ingest::error::AdmissionError;
use crate::recycled::Recycled;
use crate::scratch_pool::ScratchPool;
use common::Size2us;

/// A frame is mono or RGB — the two shapes `LinearImage` has — or a mosaic of at most three
/// colors, so it never carries more planes, and the output never more channels.
pub(super) const MAX_CHANNELS: usize = 3;
/// Output bands per rayon worker — enough for work-stealing to even out bands that different
/// amounts of the input reach.
const BANDS_PER_WORKER: usize = 4;

/// Every output plane restricted to one contiguous span of the grid: a band of rows while a frame
/// is scattered in.
#[derive(Debug)]
struct PlaneSpan<'a> {
    /// Weighted flux per channel.
    data: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    /// `Σw` and `Σw²`: one plane every channel shares, or one per color of a mosaic.
    weight: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    weight_sq: ArrayVec<&'a mut [f32], MAX_CHANNELS>,
    flags: Option<&'a mut [u8]>,
}

/// The drizzle of a run's frames, built once from the run's shape and kernel: its grid, its
/// kernel's constants and the scratch every frame's scatter reuses.
#[derive(Debug)]
pub(crate) struct Drizzler {
    input_desc: PlanarDesc,
    deposit: Deposit,
    grid: OutputGrid,
    plan: KernelPlan,
    /// The input pixels each band scans, recomputed per frame.
    scans: Vec<BandScan>,
    /// The allocation of the bands each frame is scattered by, empty between frames.
    bands: Recycled<OutputBand<'static>>,
    radial: ScratchPool<RadialScratch>,
    lattice: ScratchPool<CornerLattice>,
}

/// The drops scattered onto the output grid: `Σw·x` per channel, `Σw` and `Σw²`, and the flags the
/// drops carried.
#[derive(Debug)]
pub(crate) struct OutputSums {
    /// Accumulated weighted flux values (`Σ fluxᵢ·wᵢ`), one Buffer2 per output channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated drizzle weight `Σ wᵢ` per output pixel. The per-pixel `wᵢ` is purely geometric
    /// × pixel weight, so one plane serves every channel a drop reaches; a mosaic's photosites
    /// reach one channel each, so it keeps one per color.
    weight: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// `Σ wᵢ²` per output pixel, in the planes of `weight`, whose ratio to `(Σwᵢ)²` is the drops'
    /// Kish size.
    weight_sq: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// The [`QualityFlags::RESAMPLE_CARRIED`] flags each output pixel's drops carried; allocated
    /// when a frame carrying any arrives.
    flags: Option<Buffer2<u8>>,
}

/// One frame drizzled on its own, as the combine reads it: where its drops landed, each channel
/// their weighted mean `Σw·x/Σw`, with their summed weight `Σw` and their Kish size `(Σw)²/Σw²` —
/// one pair every channel shares, or one per color of a mosaic — and the flags they carried.
/// Where no drop landed in a channel, or the drops' weights sum to none, its pixels, weight and
/// size are 0, and nowhere else.
#[derive(Debug)]
pub(crate) struct DrizzledPlanes {
    pub(crate) pixels: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    pub(crate) drops: ArrayVec<DropPlanes<Buffer2<f32>>, MAX_CHANNELS>,
    pub(crate) flags: Option<PixelFlags>,
    /// Input pixels whose position the warp's SIP correction could not invert.
    pub(crate) unconverged_points: usize,
}

impl Drizzler {
    /// The drizzle of frames shaped as `image` — of its dimensions, and its samples reaching the
    /// output as its own do — onto `grid` by `plan`.
    pub(crate) fn new(image: &impl StackableImage, plan: KernelPlan, grid: OutputGrid) -> Self {
        let input_desc = image.desc();
        Self {
            input_desc,
            deposit: Deposit::of(image),
            grid,
            plan,
            scans: Vec::new(),
            bands: Recycled::default(),
            radial: ScratchPool::default(),
            lattice: ScratchPool::new(input_desc.width()),
        }
    }

    /// Frame `idx` drizzled on its own: see [`DrizzledPlanes`].
    ///
    /// # Errors
    ///
    /// As [`Self::scatter`].
    pub(crate) fn drizzle(
        &mut self,
        frame: &DrizzleFrame<impl StackableImage>,
        idx: usize,
    ) -> Result<DrizzledPlanes, DrizzleError> {
        let mut sums = self.sums();
        let unconverged_points = self.scatter(frame, idx, &mut sums)?;
        Ok(sums.into_planes(self.deposit, unconverged_points))
    }

    /// Nothing scattered yet, on the grid.
    pub(crate) fn sums(&self) -> OutputSums {
        let output = self.grid.size();
        let planes = |count: usize| -> ArrayVec<_, MAX_CHANNELS> {
            (0..count).map(|_| Buffer2::new_default(output)).collect()
        };
        OutputSums {
            data: planes(self.deposit.output_layout().count()),
            weight: planes(self.deposit.weight_planes()),
            weight_sq: planes(self.deposit.weight_planes()),
            flags: None,
        }
    }

    /// Scatter frame `idx`'s drops into `sums`, and count its input pixels whose position the
    /// warp's SIP correction could not invert.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame's pixel weights are of another size than it, negative or
    /// not finite. `sums` is unchanged on error.
    pub(crate) fn scatter(
        &mut self,
        frame: &DrizzleFrame<impl StackableImage>,
        idx: usize,
        sums: &mut OutputSums,
    ) -> Result<usize, DrizzleError> {
        let band_rows = Self::balanced_band_rows(self.grid.size().height);
        self.scatter_in_bands(frame, idx, sums, band_rows)
    }

    /// [`Self::scatter`] over output bands of `band_rows` rows each. The result does not depend
    /// on the band height, which the band-invariance test holds to.
    fn scatter_in_bands(
        &mut self,
        frame: &DrizzleFrame<impl StackableImage>,
        idx: usize,
        sums: &mut OutputSums,
        band_rows: usize,
    ) -> Result<usize, DrizzleError> {
        self.validate(frame, idx)?;
        let Size2us { width, height } = self.grid.size();
        if sums.flags.is_none()
            && frame
                .source
                .flags()
                .is_some_and(|flags| flags.contains(QualityFlags::RESAMPLE_CARRIED))
        {
            sums.flags = Some(Buffer2::new_default((width, height)));
        }
        let source = FrameSource::new(frame, self.deposit, self.grid);
        let plan = self.plan;
        let reach = plan.reach();
        self.scans.clear();
        self.scans
            .extend((0..height).step_by(band_rows).map(|start| {
                source.band_scan(&(start..(start + band_rows).min(height)), width, reach)
            }));

        let Self {
            ref scans,
            ref radial,
            ref lattice,
            bands: ref mut kept_bands,
            ..
        } = *self;
        let mut bands: Vec<OutputBand<'_>> = kept_bands.take();
        let span_len = width * band_rows;
        let mut data = band_chunks(&mut sums.data, span_len);
        let mut weight = band_chunks(&mut sums.weight, span_len);
        let mut weight_sq = band_chunks(&mut sums.weight_sq, span_len);
        let mut flags = sums
            .flags
            .as_mut()
            .map(|flags| flags.pixels_mut().chunks_mut(span_len));
        bands.extend((0..height.div_ceil(band_rows)).map(|band| {
            let start = band * band_rows;
            OutputBand {
                rows: start..(start + band_rows).min(height),
                width,
                planes: PlaneSpan {
                    data: next_band(&mut data),
                    weight: next_band(&mut weight),
                    weight_sq: next_band(&mut weight_sq),
                    flags: flags
                        .as_mut()
                        .map(|chunks| chunks.next().expect("one chunk per band")),
                },
                idx: band,
                scans,
                radial,
                lattice,
            }
        }));
        let unconverged = par::drain(&mut bands, ..)
            .map(|mut band| band.distribute(&source, plan))
            .sum::<usize>();
        kept_bands.put(bands);
        Ok(unconverged)
    }

    /// Refuse a frame whose pixel weights do not fit it; debug-assert what admission already
    /// checked of frame `idx`.
    fn validate(
        &self,
        frame: &DrizzleFrame<impl StackableImage>,
        idx: usize,
    ) -> Result<(), DrizzleError> {
        // Admitted before it reached here, as every frame of a drizzle is: of the set's shape and
        // pattern, its samples finite.
        debug_assert_eq!(
            frame.source.desc(),
            self.input_desc,
            "drizzle frame {idx} reached the accumulator unadmitted: another shape"
        );
        debug_assert_eq!(
            Deposit::of(&frame.source),
            self.deposit,
            "drizzle frame {idx} reached the accumulator unadmitted: another pattern"
        );
        debug_assert!(
            (0..self.input_desc.channel_count()).all(|channel| frame
                .source
                .channel(channel)
                .iter()
                .all(|v| v.is_finite())),
            "drizzle frame {idx} reached the accumulator unadmitted: a non-finite sample"
        );

        let Some(pixel_weights) = &frame.pixel_weight_map else {
            return Ok(());
        };
        let actual = pixel_weights.size();
        if actual != self.input_desc.size() {
            return Err(StackError::PlaneDimensionMismatch {
                idx,
                plane: FramePlane::PixelWeight,
                expected: self.input_desc.size(),
                actual,
            }
            .into());
        }
        if let Some((pixel, &value)) = par::iter(pixel_weights.pixels())
            .enumerate()
            .find_first(|(_, value)| !FramePlane::PixelWeight.accepts(**value))
        {
            return Err(AdmissionError::InvalidQualityPlaneValue {
                idx,
                plane: FramePlane::PixelWeight,
                pixel,
                value,
            }
            .into());
        }
        Ok(())
    }

    /// How many output rows one band covers.
    ///
    /// Several per worker so rayon can steal: a band's cost varies with how much of the input
    /// actually reaches it, which is not uniform once the transform rotates. Not tuned against the
    /// margin — over-scanning at a band boundary is nearly free (see `FrameSource::band_scan`), so
    /// there is nothing to trade off against balance.
    fn balanced_band_rows(output_height: usize) -> usize {
        let target = par::threads() * BANDS_PER_WORKER;
        output_height.div_ceil(target.max(1)).max(1)
    }
}

impl OutputSums {
    /// The sums as one drizzled frame of `deposit`'s planes, whose scatter left
    /// `unconverged_points` input pixels unplaced: see [`DrizzledPlanes`].
    pub(crate) fn into_planes(self, deposit: Deposit, unconverged_points: usize) -> DrizzledPlanes {
        let Self {
            mut data,
            weight,
            weight_sq,
            flags,
        } = self;
        let drops: ArrayVec<_, MAX_CHANNELS> = weight
            .into_iter()
            .zip(weight_sq)
            .map(|(mut weight, mut confidence)| {
                // In place, `Σw²` becoming the Kish size. A pixel whose drops sum to no positive
                // weight — a Lanczos lobe's — or whose size underflows holds nothing the combine
                // could divide its noise by, so it is one no drop reached: the weight and the size
                // are 0 together, which is the pairing `FrameQuality` documents.
                par::iter_mut(weight.pixels_mut())
                    .zip(par::iter_mut(confidence.pixels_mut()))
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
            par::iter_mut(plane.pixels_mut())
                .zip(par::iter(weight.pixels()))
                .for_each(|(value, &weight)| {
                    *value = if weight > 0.0 { *value / weight } else { 0.0 };
                });
        }
        DrizzledPlanes {
            pixels: data,
            drops,
            flags: flags.and_then(PixelFlags::from_bytes),
            unconverged_points,
        }
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

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
pub(crate) mod internals {
    use crate::drizzle::accumulator::*;
    use crate::drizzle::drizzle_config::DrizzleConfig;
    use crate::image::flags::quality_map::QualityMap;
    use crate::image::linear_image::LinearImage;
    use crate::registration::transform::Transform;
    use crate::registration::warp_transform::WarpTransform;
    use crate::stack_product::StackProduct;
    use crate::stack_product::run_report::RunReport;
    use crate::stack_product::weight_kind::WeightKind;

    /// The single-pass drizzle a test holds the per-frame one to: every frame added scattered into
    /// one set of sums.
    #[derive(Debug)]
    pub(crate) struct DrizzleAccumulator {
        drizzler: Drizzler,
        sums: OutputSums,
        config: DrizzleConfig,
        /// The run's index of the next frame added, which an error names.
        next_frame: usize,
        unconverged_points: usize,
    }

    impl DrizzleAccumulator {
        /// An empty accumulator for frames shaped as `image`, whose first frame is the run's frame
        /// `first_frame`.
        ///
        /// # Errors
        ///
        /// Returns an error when `config` is invalid.
        pub(crate) fn new(
            image: &impl StackableImage,
            config: DrizzleConfig,
            first_frame: usize,
        ) -> Result<Self, DrizzleError> {
            config.validate()?;
            let grid = OutputGrid::new(config.scale, image.desc().size());
            let plan = KernelPlan::new(config.kernel, config.pixfrac, config.scale);
            let drizzler = Drizzler::new(image, plan, grid);
            let sums = drizzler.sums();
            Ok(Self {
                drizzler,
                sums,
                config,
                next_frame: first_frame,
                unconverged_points: 0,
            })
        }

        /// Scatter `frame` into the sums.
        ///
        /// # Errors
        ///
        /// As [`Drizzler::scatter`].
        pub(crate) fn add_frame(
            &mut self,
            frame: &DrizzleFrame<impl StackableImage>,
        ) -> Result<(), DrizzleError> {
            self.unconverged_points +=
                self.drizzler
                    .scatter(frame, self.next_frame, &mut self.sums)?;
            self.next_frame += 1;
            Ok(())
        }

        /// Input pixels, over every frame added, whose position a warp's SIP correction could not
        /// invert.
        pub(crate) const fn unconverged_points(&self) -> usize {
            self.unconverged_points
        }

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

        /// [`Self::add_frame`] with the output band height pinned, so a test can compare band
        /// counts.
        pub(crate) fn add_frame_with_band_rows(
            &mut self,
            frame: &DrizzleFrame<LinearImage>,
            band_rows: usize,
        ) {
            self.unconverged_points += self
                .drizzler
                .scatter_in_bands(frame, self.next_frame, &mut self.sums, band_rows)
                .expect("test frame must be coherent with the accumulator");
            self.next_frame += 1;
        }

        /// `Σw` per output pixel as deposited, in the plane every channel shares.
        ///
        /// # Panics
        /// For a mosaic's accumulator, whose weights are per color.
        pub(crate) fn accumulated_weights(&self) -> &Buffer2<f32> {
            assert_eq!(self.sums.weight.len(), 1, "the weights are per color");
            &self.sums.weight[0]
        }

        /// `Σ flux·w` over channel `channel`'s accumulated plane, summed in f64.
        pub(crate) fn accumulated_flux_sum(&self, channel: usize) -> f64 {
            self.sums.data[channel]
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
            let deposit = self.drizzler.deposit;
            let output = self.drizzler.grid.size();
            let mut data = self.sums.data;
            let mut weight = self.sums.weight;
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
            let desc = PlanarDesc::new(output, deposit.output_layout());
            let weight = match deposit {
                Deposit::Mosaic(_) => QualityMap::PerChannel(
                    weight
                        .into_inner()
                        .expect("a mosaic's three colors each keep a weight plane"),
                ),
                Deposit::Channels(_) => {
                    QualityMap::Shared(weight.into_iter().next().expect("one shared plane"))
                }
            };
            StackProduct {
                image: LinearImage::from_planar_channels(
                    desc,
                    data.into_iter().map(Buffer2::into_vec),
                ),
                coverage: None,
                weight: Some(weight),
                weight_kind: WeightKind::Count,
                inverse_variance: None,
                dispersion: None,
                cfa_type: None,
                report: RunReport::default(),
            }
        }
    }
}
