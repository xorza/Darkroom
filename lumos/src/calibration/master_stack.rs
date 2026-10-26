//! [`MasterStack`]: one calibration role's raw CFA frames, stacked into its master.

use std::path::Path;

use crate::calibration::error::MasterStackError;
use crate::calibration::master_role::MasterRole;
use crate::calibration::master_subtraction::MasterSubtraction;
use crate::calibration::subtractor::Subtractor;
use crate::combine::frame_cache::FrameCache;
use crate::combine::frame_cache::frame_step::SteppedPaths;
use crate::combine::stack::planned_combine::PlannedCombine;
use crate::combine::stack::{CombineSource, FrameSet};
use crate::combine::stack_config::StackConfig;
use crate::frame::capture_conditions::CaptureConditions;
use crate::image::cfa::cfa_image::CfaImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::stack_product::quality_planes::QualityPlanes;

/// One calibration role's raw CFA frames, to stack into a single master under `config` — the
/// role's preset is [`MasterRole::stack_config`] — read as `ingest` says.
/// [`CalibrationMasters::build`](crate::CalibrationMasters::build) stacks every role this way.
///
/// `subtract` is taken from every frame, in order, before its statistics and the combine, and each
/// frame's record of what calibration removed takes what each subtractor still held: flats take
/// their bias and their flat-dark's dark signal this way, so the multiplicative normalization
/// scales each flat's own signal. Scaling a flat that still holds its offset `b` and subtracting
/// the offset from the master afterwards leaves `b·(mean gain − 1)`, a vignetting residual of a few
/// percent, as PixInsight and Siril avoid by calibrating each flat first. A subtractor holding dark
/// signal is matched to each frame's exposure and temperature as a dark is to a light.
///
/// The master states the exposure and temperature its frames share. The frames of a dark or a
/// flat-dark have to share them: its signal is one exposure's at one temperature.
///
/// The preset carries its own small-frame fallback (`Combine::small_n`): the combine engine
/// downgrades to the median below the preset's `min_frames` (e.g. `flat()` below 8), so no
/// frame-count special-casing is needed here.
#[derive(Debug)]
pub(crate) struct MasterStack<'a, P> {
    pub(crate) paths: &'a [P],
    pub(crate) role: MasterRole,
    pub(crate) config: StackConfig,
    pub(crate) ingest: &'a IngestConfig,
    pub(crate) subtract: &'a [Subtractor<'a>],
}

impl<P: AsRef<Path>> MasterStack<'_, P> {
    /// The master, or `None` when there are no `paths`.
    ///
    /// # Errors
    ///
    /// A [`MasterStackError`]: the subtractor does not fit the role, a frame does not fit the
    /// subtractor, a dark's frames disagree on their exposure, or the stack itself fails. Frames
    /// that disagree on their sensor temperature stack, warned of, into a master that states none.
    pub(crate) fn run(
        self,
        monitor: ProgressMonitor,
    ) -> Result<Option<CfaImage>, MasterStackError> {
        let Self {
            paths,
            role,
            config,
            ingest,
            subtract,
        } = self;
        // `None` rather than `StackError::NoFrames`: an absent calibration role is normal, and this
        // is the one thing `combine_cached` cannot decide for us.
        if paths.is_empty() {
            return Ok(None);
        }
        let subtraction = MasterSubtraction::new(role, subtract)?;
        let run = IngestRun::new(ingest, monitor);
        // A master is mosaic data for the calibration stage to consume, not a science product: the
        // ancillary planes would be allocated and written per pixel for nothing.
        let config = StackConfig {
            quality: QualityPlanes::IMAGE_ONLY,
            ..config
        };
        let set = FrameSet {
            source: CombineSource::CfaPaths,
            count: paths.len(),
        };
        let product = config.combine_cached(set, || {
            let cache = FrameCache::from_cfa_paths::<_, _, MasterStackError>(
                SteppedPaths {
                    paths,
                    step: &subtraction,
                },
                PlannedCombine {
                    normalization: config.normalization,
                    quality: config.quality,
                },
                run,
            )?;
            if role.is_dark() {
                let conditions = cache.frames.iter().map(|frame| frame.facts.conditions);
                CaptureConditions::check_exposures(conditions.clone())
                    .map_err(|source| MasterStackError::Conditions { role, source })?;
                if let Some(disagreement) = CaptureConditions::temperature_disagreement(conditions)
                {
                    tracing::warn!(
                        %role,
                        %disagreement,
                        "the frames of the master disagree on their sensor temperature; it states \
                         none"
                    );
                }
            }
            Ok::<_, MasterStackError>(cache)
        })?;

        Ok(Some(product.into_cfa_master()))
    }
}
