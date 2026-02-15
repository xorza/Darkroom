//! Calibration master frame creation and management.

pub mod defect_map;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::astro_image::cfa::CfaImage;
use crate::stacking::FrameType;
use crate::stacking::cache::ImageCache;
use crate::stacking::config::StackConfig;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::stack::run_stacking;

pub use defect_map::DefectMap;

/// Default sigma threshold for hot pixel detection.
///
/// A pixel is flagged as defective if it deviates from the per-color median
/// by more than `sigma_threshold × σ` (where σ is estimated from MAD).
/// PixInsight uses 3.0; 5.0 is more conservative (fewer false positives).
pub const DEFAULT_HOT_PIXEL_SIGMA: f32 = 5.0;

/// Holds master calibration frames (dark, flat, bias) and hot pixel map.
///
/// Operates on raw CFA (single-channel sensor) data before demosaicing,
/// giving mathematically correct hot pixel correction using same-color
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
    /// Hot pixel map derived from master dark
    pub defect_map: Option<DefectMap>,
}

/// Stack raw CFA frames using the full stacking pipeline.
///
/// Uses the frame-type preset config. Falls back to median for < 8 frames
/// (too few for rejection to work well).
/// Returns `None` if `paths` is empty.
fn stack_cfa_frames(
    paths: &[impl AsRef<Path> + Sync],
    frame_type: FrameType,
    config: StackConfig,
) -> Result<Option<CfaImage>, crate::stacking::Error> {
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

    let cache = ImageCache::<CfaImage>::from_paths(
        paths,
        &config.cache,
        frame_type,
        ProgressCallback::default(),
    )?;

    let result = run_stacking(&cache, &config);

    Ok(Some(result))
}

impl CalibrationMasters {
    /// Create CalibrationMasters from pre-built CFA images.
    ///
    /// Generates hot pixel map from the CFA dark if provided.
    /// `flat_dark` is a dark frame taken at the flat's exposure time — used instead
    /// of bias for flat normalization when provided.
    /// `hot_pixel_sigma` controls defect detection sensitivity (see
    /// [`DEFAULT_HOT_PIXEL_SIGMA`]).
    pub fn new(
        dark: Option<CfaImage>,
        flat: Option<CfaImage>,
        bias: Option<CfaImage>,
        flat_dark: Option<CfaImage>,
        hot_pixel_sigma: f32,
    ) -> Self {
        let defect_map = dark
            .as_ref()
            .map(|d| DefectMap::from_master_dark(d, hot_pixel_sigma));

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
    /// Empty slices produce `None` for that master.
    ///
    /// `flat_darks` are dark frames taken at the flat exposure time. When provided,
    /// they are subtracted from the flat instead of bias during normalization.
    /// Important for narrowband imaging where flat exposures accumulate dark current.
    /// `hot_pixel_sigma` controls defect detection sensitivity (see
    /// [`DEFAULT_HOT_PIXEL_SIGMA`]).
    pub fn from_raw_files(
        darks: &[impl AsRef<Path> + Sync],
        flats: &[impl AsRef<Path> + Sync],
        biases: &[impl AsRef<Path> + Sync],
        flat_darks: &[impl AsRef<Path> + Sync],
        hot_pixel_sigma: f32,
    ) -> Result<Self, crate::stacking::Error> {
        let dark = stack_cfa_frames(darks, FrameType::Dark, StackConfig::dark())?;
        let flat = stack_cfa_frames(flats, FrameType::Flat, StackConfig::flat())?;
        let bias = stack_cfa_frames(biases, FrameType::Bias, StackConfig::bias())?;
        let flat_dark = stack_cfa_frames(flat_darks, FrameType::Dark, StackConfig::dark())?;
        Ok(Self::new(dark, flat, bias, flat_dark, hot_pixel_sigma))
    }

    /// Calibrate a raw CFA light frame in place.
    ///
    /// Applies calibration on raw (un-demosaiced) data:
    /// 1. Dark subtraction (or bias if no dark)
    /// 2. Flat division with normalization
    /// 3. CFA-aware hot pixel correction
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
