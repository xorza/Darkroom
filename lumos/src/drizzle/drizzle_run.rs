//! [`DrizzleRun`]: one drizzle under way, the body of the drizzle entries of [`DrizzleConfig`]:
//! from paths, or from frames already in memory, of demosaiced or mono frames or of calibrated
//! mosaics.
//!
//! Each frame is drizzled onto the output grid on its own, as Siril does: its drops' weighted mean,
//! their summed weight and their Kish size, the noise factor of the mean. The drizzled frames then
//! go through the combine every stack takes — normalization, weighting, rejection and the quality
//! planes — with each frame's drop weight multiplying its frame weight in the mean, as Siril's
//! (`median_and_mean.c`, `n *= dstack[frame]`) and DrizzlePac's weights do. Without rejection, and
//! with equal frame weights, that is the single-pass drizzle.
//!
//! A mosaic drizzles each photosite into the channel of its color alone, so its color planes come
//! out with no color interpolated from another: a demosaic, and the second interpolation a warp
//! would add, are both skipped. Each channel then has its own weights, and the combine gathers it
//! by them.
//!
//! The path form keeps one decoded input resident: a frame is loaded, drizzled, stored, and
//! dropped before the next is read.

use std::iter;
use std::path::Path;
use std::sync::Arc;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::combine::error::StackError;
use crate::combine::frame_cache::core::CacheCore;
use crate::drizzle::accumulator::kernel_plan::KernelPlan;
use crate::drizzle::accumulator::{DrizzledPlanes, Drizzler, MAX_CHANNELS};
use crate::drizzle::deposit::Deposit;
use crate::drizzle::drizzle_config::DrizzleConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::drizzle_shape::{DrizzleShape, FrameOrigin};
use crate::drizzle::error::DrizzleError;
use crate::drizzle::output_grid::OutputGrid;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::run_storage::RunStorage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::DemosaicProvenance;
use crate::image::linear_image::LinearImage;
use crate::image::stackable_image::StackableImage;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::Decoder;
use crate::io::load_context::LoadContext;
use crate::progress::stacking_progress::StackingStage;
use crate::stack_product::StackProduct;

/// One drizzle under way: what it was asked to do, and the memory reading, decode policy, cancel
/// token and progress it took at its entry.
#[derive(Debug)]
pub(super) struct DrizzleRun<'a> {
    pub(super) config: &'a DrizzleConfig,
    pub(super) ingest: IngestRun,
}

/// `frame`'s source decoded by `D` under `context`, through `storage`.
fn load_drizzle_frame<D: Decoder, P: AsRef<Path>>(
    frame: DrizzleFrame<P>,
    storage: &RunStorage,
    context: &LoadContext,
) -> Result<DrizzleFrame<D::Image>, DrizzleError> {
    let DrizzleFrame {
        source,
        warp,
        pixel_weight_map,
    } = frame;
    Ok(DrizzleFrame {
        source: storage.decode::<D>(source.as_ref(), context)?,
        warp,
        pixel_weight_map,
    })
}

impl DrizzleRun<'_> {
    /// The body of the path entries of [`DrizzleConfig`]: each frame decoded by `D` when its turn
    /// comes.
    pub(super) fn drizzle_files<D: Decoder, P: AsRef<Path>>(
        &self,
        frames: Vec<DrizzleFrame<P>>,
    ) -> Result<StackProduct, DrizzleError> {
        let run = &self.ingest;
        let context = run.context.clone();
        let storage = RunStorage::decoding(run)?;
        let sources: Vec<&Path> = frames.iter().map(|frame| frame.source.as_ref()).collect();
        let origin = FrameOrigin::Files {
            uncached: storage.uncached::<D>(&sources, &context),
        };
        // Lazy, so only the frame currently being drizzled is resident.
        let loaded = frames
            .into_iter()
            .map(|frame| load_drizzle_frame::<D, P>(frame, &storage, &context));
        self.accumulate(loaded, origin, &storage)
    }

    /// The body of the in-memory entries of [`DrizzleConfig`].
    pub(super) fn drizzle_held<I: StackableImage>(
        &self,
        frames: Vec<DrizzleFrame<I>>,
    ) -> Result<StackProduct, DrizzleError> {
        self.accumulate(
            frames.into_iter().map(Ok),
            FrameOrigin::Memory,
            &RunStorage::holding(&self.ingest),
        )
    }

    /// Drizzle every frame on its own, store it, combine the stored frames, and gate the product —
    /// the body every entry point shares.
    ///
    /// Frames arrive as a lazy fallible iterator so the path entries keep one input image resident:
    /// an item is produced, drizzled, stored, and dropped before the next is loaded.
    fn accumulate<I: StackableImage>(
        &self,
        mut frames: impl ExactSizeIterator<Item = Result<DrizzleFrame<I>, DrizzleError>>,
        origin: FrameOrigin,
        storage: &RunStorage,
    ) -> Result<StackProduct, DrizzleError> {
        let frame_count = frames.len();
        if frame_count == 0 {
            return Err(StackError::NoFrames.into());
        }
        let Self {
            config: drizzle,
            ingest: run,
        } = self;
        let stack = &drizzle.stack;
        let cancel = &run.context.cancel;
        // Before the first frame is pulled: for the path entry that is a full decode, spent on a
        // run the configuration already dooms.
        drizzle.validate()?;
        stack.validate(frame_count).map_err(StackError::from)?;

        let first = frames.next().expect("frame_count is non-zero")?;
        let input_dims = first.source.dimensions();
        let deposit = Deposit::of(&first.source);
        tracing::info!(
            ?origin,
            frame_count,
            input_width = input_dims.width(),
            input_height = input_dims.height(),
            channels = input_dims.channels(),
            mosaic = ?deposit.mosaic(),
            output_scale = drizzle.scale,
            pixfrac = drizzle.pixfrac,
            kernel = ?drizzle.kernel,
            "Starting drizzle stacking"
        );
        let grid = OutputGrid::new(drizzle.scale, input_dims.size());
        let output = ImageDimensions::new(grid.size(), deposit.output_layout());
        let mut drizzler = Drizzler::new(
            &first.source,
            KernelPlan::new(drizzle.kernel, drizzle.pixfrac, drizzle.scale),
            grid,
        );
        let tier = DrizzleShape {
            input: input_dims,
            output,
            deposit,
            frame_count,
            origin,
        }
        .tier(stack, run, storage)?;
        // Measured on each frame as it arrives, before its drops are spread: the drizzled pixels
        // average several, and would understate it. A mosaic's are measured per color, which its
        // drizzle makes the channels, with the spread its per-frame normalization scales by.
        let intake = FrameIntake::new(
            IntakePlan {
                dimensions: input_dims,
                frame_count,
                decode_slots: 1,
                stage: StackingStage::Drizzling,
            },
            run,
        );
        let intake = match deposit {
            Deposit::Mosaic(_) => intake.measuring_spread(),
            Deposit::Channels(_) => intake,
        };

        let mut stored = Vec::with_capacity(frame_count);
        // Every frame's drops' weight at each output pixel, before any frame weight, in the planes
        // of their weights: how much signal landed there, which the fill gate reads.
        let mut depth: ArrayVec<Buffer2<f32>, MAX_CHANNELS> = (0..deposit.weight_planes())
            .map(|_| Buffer2::new_default(output.width(), output.height()))
            .collect();
        let mut unconverged_points = 0;
        let mut metadata = None;
        for (index, frame) in iter::once(Ok(first)).chain(frames).enumerate() {
            // Between frames, so a cancelled run stops before loading and drizzling the next one.
            if cancel.is_cancelled() {
                return Err(StackError::Cancelled.into());
            }
            let frame = frame?;
            let source_stats = intake.admit(index, &frame.source)?.into_drizzled();
            let DrizzledPlanes {
                pixels,
                drops,
                flags,
                unconverged_points: unplaced,
            } = drizzler.drizzle(&frame, index)?;
            unconverged_points += unplaced;
            // Each output pixel sums the drops of several source pixels.
            let mut frame_metadata = frame.source.metadata().clone();
            frame_metadata.derived.after_resample();
            let flat_gain = frame.source.flat_gain().map(|gain| {
                Arc::new(gain.warped(
                    |position| frame.warp.apply(grid.reference_point(position)),
                    output.size(),
                ))
            });
            if let Deposit::Mosaic(cfa_type) = deposit {
                color_planes(&mut frame_metadata, cfa_type);
            }
            drop(frame);
            for (depth, drops) in depth.iter_mut().zip(&drops) {
                depth
                    .pixels_mut()
                    .par_iter_mut()
                    .zip(drops.weight.pixels().par_iter())
                    .for_each(|(depth, &weight)| *depth += weight);
            }
            metadata.get_or_insert_with(|| frame_metadata.clone());
            let mut image = LinearImage::from_planar_channels(
                output,
                pixels.into_iter().map(Buffer2::into_vec),
            );
            image.metadata = frame_metadata;
            image.set_flags(flags);
            image.set_flat_gain(flat_gain);
            stored.push(
                tier.store(image, FrameQuality::Drizzled { drops }, source_stats)
                    .map_err(DrizzleError::from)?,
            );
            intake.complete_one();
        }

        let mut product = stack.stack_stored_frames(
            stored,
            CacheCore {
                tier,
                dimensions: output,
                metadata: metadata.expect("a frame was drizzled"),
                monitor: run.monitor(),
            },
        )?;
        // A pixel below the gate is held at the fill value: what the frames left there is too thin
        // a share of the deepest pixel's to stand. Floored at the smallest positive float, so
        // "reached at all" and "reached enough" are one comparison. Each weight plane against its
        // own deepest: a mosaic's green photosites outnumber its red and blue ones, and each color
        // plane is gated as the image of its own photosites.
        let thresholds: ArrayVec<f32, MAX_CHANNELS> = depth
            .iter()
            .map(|depth| {
                let deepest = depth.pixels().iter().copied().fold(0.0f32, f32::max);
                (drizzle.min_weight_fraction * deepest).max(f32::MIN_POSITIVE)
            })
            .collect();
        product.fill_where(
            |channel, index| {
                let plane = deposit.weight_plane(channel);
                depth[plane].pixels()[index] < thresholds[plane]
            },
            drizzle.fill_value,
        );
        product.report.unconverged_points =
            u64::try_from(unconverged_points).expect("a pixel count fits u64");
        Ok(product)
    }
}

/// `metadata`, of a `cfa_type` mosaic, as the metadata of the color planes its drizzle makes:
/// sensor color with nothing interpolated.
const fn color_planes(metadata: &mut ImageMetadata, cfa_type: CfaType) {
    if let Some(provenance) = metadata.derived.provenance_mut() {
        provenance.color = cfa_type.planes_color();
        provenance.demosaic = DemosaicProvenance::CfaDrizzle;
    }
}
