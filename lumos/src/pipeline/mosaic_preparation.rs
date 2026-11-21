//! [`MosaicPreparation`]: what a mosaic light goes through before its stars are detected.

use std::path::Path;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::cosmic_ray_detector::CosmicRayDetector;
use crate::error::Cancelled;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::linear_image::LinearImage;
use crate::io::load_ctx::LoadCtx;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;
use crate::pipeline::calibration_notes::CalibrationNotes;
use crate::pipeline::error::AlignStackError;

/// What a mosaic light goes through before its stars are detected.
#[derive(Debug)]
pub(super) struct MosaicPreparation<'a> {
    pub(super) masters: &'a CalibrationMasters,
    pub(super) cosmic_ray: Option<&'a CosmicRayConfig>,
    /// What calibrating the lights could not check, counted for the run's report.
    pub(super) notes: &'a CalibrationNotes,
}

/// Where a mosaic light came from and what its preparation works with: the path it is named by,
/// the decode policy and cancel token, and the worker's cosmic-ray detector.
#[derive(Debug)]
pub(super) struct MosaicLight<'a> {
    pub(super) path: &'a Path,
    pub(super) ctx: &'a LoadCtx,
    pub(super) cosmic_ray: &'a mut CosmicRayDetector,
}

impl MosaicPreparation<'_> {
    /// `cfa`, the light decoded from `light.path`, calibrated by the masters, cleaned of cosmic
    /// rays when asked, and demosaiced.
    pub(super) fn prepare(
        &self,
        mut cfa: CfaImage,
        light: MosaicLight<'_>,
    ) -> Result<LinearImage, AlignStackError> {
        let MosaicLight {
            path,
            ctx,
            cosmic_ray: detector,
        } = light;
        let outcome =
            self.masters
                .calibrate(&mut cfa)
                .map_err(|source| AlignStackError::Calibration {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })?;
        self.notes.record(outcome);
        if let Some(cosmic_ray) = self.cosmic_ray {
            let removed = detector.reject(&mut cfa, cosmic_ray).map_err(|source| {
                AlignStackError::CosmicRay {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            tracing::info!(removed, "rejected cosmic rays");
        }
        let noise = MosaicNoiseMeasure::of(&cfa).map(MosaicNoiseMeasure::measure);
        // The demosaic polls the cancel token between its passes.
        cfa.demosaic(ctx.decode.xtrans_passes, noise, &ctx.cancel)
            .map_err(|Cancelled| AlignStackError::Cancelled)
    }
}
