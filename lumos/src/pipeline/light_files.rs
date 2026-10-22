//! [`LightFiles`]: lights read from files, and what each goes through before its stars are
//! detected.

use std::path::Path;

use crate::calibration::cosmic_ray;
use crate::frame::frame_peek::FramePeek;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::file_set::{FileSet, Loaded, SetPeek};
use crate::frame_store::run_storage::RunStorage;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::cfa_decoder::CfaDecoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::io::load_ctx::LoadCtx;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::light_shape::LightShape;
use crate::pipeline::mosaic_preparation::MosaicPreparation;

/// Lights read from files, each through what it goes through before its stars are detected, as
/// the decoder that preparation reads them with takes them.
#[derive(Debug)]
pub(super) enum LightFiles<'a, P> {
    /// Mosaics, read as such and prepared.
    Calibrate {
        files: FileSet<'a, P, CfaDecoder>,
        mosaic: MosaicPreparation<'a>,
    },
    /// Images already calibrated, read as they decode.
    Linear(FileSet<'a, P, LinearDecoder>),
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
        let storage = RunStorage::decoding(run)?;
        Ok(match preparation {
            Preparation::Calibrate(mosaic) => Self::Calibrate {
                files: FileSet::new(paths, storage, &run.ctx),
                mosaic,
            },
            Preparation::Linear => Self::Linear(FileSet::new(paths, storage, &run.ctx)),
        })
    }

    pub(super) const fn len(&self) -> usize {
        match self {
            Self::Calibrate { files, .. } => files.len(),
            Self::Linear(files) => files.len(),
        }
    }

    /// Where the lights are read from and written to.
    pub(super) const fn storage(&self) -> &RunStorage {
        match self {
            Self::Calibrate { files, .. } => files.storage(),
            Self::Linear(files) => files.storage(),
        }
    }

    /// The run's shape, from a light's header — or, for a linear format with none to read, its
    /// decode — which that light's preparation then takes ([`FileSet::peek`]), with the source gains
    /// the lights share and the bytes the lights the decode cache does not hold yet write to it,
    /// a gain each at the most.
    pub(super) fn shape(&self, run: &IngestRun) -> Result<LightShape, AlignStackError> {
        let workers = run.budget.workers();
        let (files, mosaic) = match self {
            Self::Calibrate { files, mosaic } => (files, mosaic),
            Self::Linear(files) => {
                let SetPeek {
                    info, held, gains, ..
                } = files.peek(&run.ctx)?;
                let peek = FramePeek::from(info);
                return Ok(LightShape {
                    kept_writes: kept_writes(
                        files.uncached(),
                        peek.resident_bytes() + info.gain_bytes,
                    ),
                    peeked: held,
                    gains,
                    ..LightShape::decoded(peek, files.len(), workers)
                });
            }
        };
        let SetPeek {
            idx,
            info,
            held,
            gains,
        } = files.peek(&run.ctx)?;
        let size = info.dimensions.size();
        // Refused here, before any light is prepared, when the pass cannot run on this sensor.
        let cosmic_ray = match mosaic.cosmic_ray {
            Some(_) => cosmic_ray::heap_bytes(&info.cfa_type, size).map_err(|source| {
                AlignStackError::CosmicRay {
                    path: files.path(idx).to_path_buf(),
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
            count: files.len(),
            dimensions,
            prepared,
            held: 0,
            peeked: held,
            gains,
            kept_writes: kept_writes(files.uncached(), info.decoded().kept + info.gain_bytes),
        })
    }

    /// Load light `idx` and make it ready: a mosaic calibrated by the masters, cleaned of cosmic
    /// rays when asked, and demosaiced, which the decode cache keeps no planes of; a linear image
    /// as it decodes, with its planes where the cache keeps them.
    pub(super) fn prepare(
        &self,
        idx: usize,
        ctx: &LoadCtx,
    ) -> Result<Loaded<LinearImage>, AlignStackError> {
        match self {
            Self::Calibrate { files, mosaic } => Ok(Loaded {
                image: mosaic.prepare(files.decode(idx, ctx)?.image, files.path(idx), ctx)?,
                kept: None,
            }),
            Self::Linear(files) => Ok(files.decode(idx, ctx)?),
        }
    }
}

/// The bytes `uncached` lights add to the decode cache, `frame_bytes` each.
const fn kept_writes(uncached: usize, frame_bytes: usize) -> u64 {
    (uncached as u64).saturating_mul(frame_bytes as u64)
}
