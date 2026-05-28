//! Calibration master frame creation and management.

pub mod defect_map;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::astro_image::cfa::CfaImage;
use crate::stacking::cache::ImageCache;
use crate::stacking::config::StackConfig;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::stack::run_stacking;
use defect_map::DefectMap;

/// Default sigma threshold for defect detection.
///
/// A pixel is flagged as defective if it deviates from the per-color median
/// by more than `sigma_threshold × σ` (where σ is estimated from MAD).
/// PixInsight uses 3.0; 5.0 is more conservative (fewer false positives).
pub const DEFAULT_SIGMA_THRESHOLD: f32 = 5.0;

/// Raw frame paths for [`CalibrationMasters::from_files`], grouped by role.
///
/// Naming each role at the call site prevents accidentally swapping, say, flats
/// and bias (all four are path slices). An empty slice produces `None` for that
/// master.
#[derive(Debug)]
pub struct CalibrationFrames<'a, P: AsRef<Path> + Sync> {
    /// Dark frames (thermal-noise calibration).
    pub darks: &'a [P],
    /// Flat frames (vignetting / dust correction).
    pub flats: &'a [P],
    /// Bias frames (read-noise calibration).
    pub bias: &'a [P],
    /// Dark frames taken at the flat exposure time. When non-empty, subtracted
    /// from the flat instead of bias during flat normalization — matters for
    /// narrowband, where flat exposures accumulate dark current.
    pub flat_darks: &'a [P],
}

/// Holds master calibration frames (dark, flat, bias) and defect map.
///
/// Operates on raw CFA (single-channel sensor) data before demosaicing,
/// giving mathematically correct defect pixel correction using same-color
/// CFA neighbors.
#[derive(Debug)]
pub struct CalibrationMasters {
    /// Master dark frame (raw CFA)
    pub master_dark: Option<CfaImage>,
    /// Master flat frame (raw CFA)
    pub master_flat: Option<CfaImage>,
    /// Master bias frame (raw CFA)
    pub master_bias: Option<CfaImage>,
    /// Master flat dark frame (dark taken at flat exposure time)
    pub master_flat_dark: Option<CfaImage>,
    /// Defect map (hot + cold pixels) derived from master dark
    pub defect_map: Option<DefectMap>,
}

/// Stack raw CFA frames using the full stacking pipeline.
///
/// Uses the frame-type preset config. Falls back to median for < 8 frames
/// (too few for rejection to work well).
/// Returns `None` if `paths` is empty.
fn stack_cfa_frames(
    paths: &[impl AsRef<Path> + Sync],
    config: StackConfig,
) -> Result<Option<CfaImage>, crate::stacking::error::Error> {
    if paths.is_empty() {
        return Ok(None);
    }

    let config = if paths.len() < 8 {
        StackConfig {
            normalization: config.normalization,
            ..StackConfig::median()
        }
    } else {
        config
    };

    let cache =
        ImageCache::<CfaImage>::from_paths(paths, &config.cache, ProgressCallback::default())?;

    let result = run_stacking(&cache, &config);

    Ok(Some(result))
}

impl CalibrationMasters {
    /// Create CalibrationMasters from pre-built CFA images.
    ///
    /// Generates defect map from the CFA dark if provided.
    /// `flat_dark` is a dark frame taken at the flat's exposure time — used instead
    /// of bias for flat normalization when provided.
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    pub fn from_images(
        dark: Option<CfaImage>,
        flat: Option<CfaImage>,
        bias: Option<CfaImage>,
        flat_dark: Option<CfaImage>,
        sigma_threshold: f32,
    ) -> Self {
        let defect_map = dark
            .as_ref()
            .map(|d| DefectMap::from_master_dark(d, sigma_threshold));

        Self {
            master_dark: dark,
            master_flat: flat,
            master_bias: bias,
            master_flat_dark: flat_dark,
            defect_map,
        }
    }

    /// Create CalibrationMasters by stacking raw CFA files.
    ///
    /// Uses sigma-clipped mean (>= 8 frames) or median (< 8 frames)
    /// with the full stacking pipeline (rejection, normalization, chunked processing).
    /// Empty slices produce `None` for that master (see [`CalibrationFrames`]).
    ///
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    pub fn from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationFrames<'_, P>,
        sigma_threshold: f32,
    ) -> Result<Self, crate::stacking::error::Error> {
        let dark = stack_cfa_frames(frames.darks, StackConfig::dark())?;
        let flat = stack_cfa_frames(frames.flats, StackConfig::flat())?;
        let bias = stack_cfa_frames(frames.bias, StackConfig::bias())?;
        let flat_dark = stack_cfa_frames(frames.flat_darks, StackConfig::dark())?;
        Ok(Self::from_images(
            dark,
            flat,
            bias,
            flat_dark,
            sigma_threshold,
        ))
    }

    /// Calibrate a raw CFA light frame in place.
    ///
    /// Applies calibration on raw (un-demosaiced) data:
    /// 1. Dark subtraction (or bias if no dark)
    /// 2. Flat division with normalization
    /// 3. CFA-aware defect pixel correction
    pub fn calibrate(&self, image: &mut CfaImage) {
        // 1. Dark subtraction
        if let Some(ref dark) = self.master_dark {
            image.subtract(dark);
        } else if let Some(ref bias) = self.master_bias {
            image.subtract(bias);
        }

        // 2. Flat division (flat dark takes priority over bias for flat normalization)
        if let Some(ref flat) = self.master_flat {
            let flat_sub = self.master_flat_dark.as_ref().or(self.master_bias.as_ref());
            image.divide_by_normalized(flat, flat_sub);
        }

        // 3. CFA-aware defective pixel correction
        if let Some(ref defect_map) = self.defect_map {
            defect_map.correct(image);
        }
    }
}
