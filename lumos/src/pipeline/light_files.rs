//! [`LightFiles`]: lights read from files, and what each goes through before its stars are
//! detected.

use std::path::Path;

use crate::calibration::cosmic_ray;
use crate::frame::frame_peek::FramePeek;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::run_storage::RunStorage;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::Decoder;
use crate::io::decoder::cfa_decoder::CfaDecoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::io::load_ctx::LoadCtx;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::light_shape::LightShape;
use crate::pipeline::mosaic_preparation::MosaicPreparation;

/// Lights read from files, what each goes through before its stars are detected, and the storage
/// they are read through.
#[derive(Debug)]
pub(super) struct LightFiles<'a, P> {
    paths: &'a [P],
    preparation: Preparation<'a>,
    storage: RunStorage,
}

/// What a light read from a file goes through before its stars are detected.
#[derive(Debug)]
pub(super) enum Preparation<'a> {
    /// A mosaic, calibrated, cleaned of cosmic rays and demosaiced.
    Calibrate(MosaicPreparation<'a>),
    /// An image already calibrated, taken as it decodes.
    Linear,
}

impl<'a, P: AsRef<Path> + Sync> LightFiles<'a, P> {
    /// The lights of `paths`, each through `preparation`, read through the storage of `run`.
    ///
    /// # Errors
    ///
    /// The decode cache cannot be opened.
    pub(super) fn new(
        paths: &'a [P],
        preparation: Preparation<'a>,
        run: &IngestRun,
    ) -> Result<Self, AlignStackError> {
        Ok(Self {
            paths,
            preparation,
            storage: RunStorage::decoding(run)?,
        })
    }

    pub(super) const fn len(&self) -> usize {
        self.paths.len()
    }

    /// Where the lights are read from and written to.
    pub(super) const fn storage(&self) -> &RunStorage {
        &self.storage
    }

    /// The run's shape, from the first file's header — or, for a linear format with none to read,
    /// its decode — with the bytes the lights the decode cache does not hold yet write to it.
    pub(super) fn shape(&self, run: &IngestRun) -> Result<LightShape, AlignStackError> {
        let first = self.paths[0].as_ref();
        let workers = run.budget.workers();
        let cosmic_ray = match self.preparation {
            Preparation::Calibrate(MosaicPreparation { cosmic_ray, .. }) => cosmic_ray,
            Preparation::Linear => {
                let peek = match LinearDecoder::peek(first, &run.ctx) {
                    Some(info) => FramePeek::from(info),
                    None => FramePeek::of_decoded(&self.decode::<LinearDecoder>(first, &run.ctx)?),
                };
                return Ok(LightShape {
                    kept_writes: self.kept_writes::<LinearDecoder>(peek.resident_bytes(), run),
                    ..LightShape::decoded(peek, self.paths.len(), workers)
                });
            }
        };
        let info = run.ctx.cfa_info(first)?;
        let size = info.dimensions.size();
        // Refused here, before any light is decoded, when the pass cannot run on this sensor.
        let cosmic_ray = match cosmic_ray {
            Some(_) => cosmic_ray::heap_bytes(&info.cfa_type, size).map_err(|source| {
                AlignStackError::CosmicRay {
                    path: first.to_path_buf(),
                    source,
                }
            })?,
            None => 0,
        };
        let dimensions = ImageDimensions::new(size, info.cfa_type.color_layout());
        // The decode; the cosmic-ray pass over the mosaic; the demosaic; the statistics, or the
        // detection's noise estimate, beside the demosaiced frame.
        let prepared = info
            .demosaiced(
                info.decoded().beside(cosmic_ray),
                run.ctx.xtrans_passes,
                workers,
            )
            .beside(FrameStats::heap_bytes(dimensions, None, workers));
        Ok(LightShape {
            count: self.paths.len(),
            dimensions,
            prepared,
            held: 0,
            kept_writes: self.kept_writes::<CfaDecoder>(info.decoded().kept, run),
        })
    }

    /// Load light `idx` and make it ready: a mosaic calibrated by the masters, cleaned of cosmic
    /// rays when asked, and demosaiced; a linear image as it decodes.
    pub(super) fn prepare(
        &self,
        idx: usize,
        ctx: &LoadCtx,
    ) -> Result<LinearImage, AlignStackError> {
        let path = self.paths[idx].as_ref();
        let Preparation::Calibrate(mosaic) = &self.preparation else {
            return self.decode::<LinearDecoder>(path, ctx);
        };
        mosaic.prepare(self.decode::<CfaDecoder>(path, ctx)?, path, ctx)
    }

    /// `path` decoded by `D` under `ctx` through the storage: a RAW light as its uncalibrated
    /// mosaic, so a later run with other masters reuses it too.
    fn decode<D: Decoder>(&self, path: &Path, ctx: &LoadCtx) -> Result<D::Image, AlignStackError> {
        Ok(self.storage.decode::<D>(path, ctx)?)
    }

    /// The bytes the lights decoded by `D` add to the decode cache of `run`, `frame_bytes` each it
    /// does not hold yet.
    fn kept_writes<D: Decoder>(&self, frame_bytes: usize, run: &IngestRun) -> u64 {
        (self.storage.uncached::<D>(self.paths, &run.ctx) as u64).saturating_mul(frame_bytes as u64)
    }
}
