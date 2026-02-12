//! Calibration master frame creation and management.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::Result;

use crate::astro_image::cfa::CfaImage;
use crate::raw::load_raw_cfa;
use crate::stacking::hot_pixels::HotPixelMap;

/// Default sigma threshold for hot pixel detection.
const DEFAULT_HOT_PIXEL_SIGMA: f32 = 5.0;

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
    /// Hot pixel map derived from master dark
    pub hot_pixel_map: Option<HotPixelMap>,
}

/// Compute pixel-wise mean of raw CFA frames loaded from files.
///
/// Returns `None` if `paths` is empty.
fn compute_cfa_mean(paths: &[impl AsRef<Path>]) -> Result<Option<CfaImage>> {
    if paths.is_empty() {
        return Ok(None);
    }

    let mut acc = load_raw_cfa(paths[0].as_ref())?;
    for path in &paths[1..] {
        let frame = load_raw_cfa(path.as_ref())?;
        for (a, b) in acc.pixels.iter_mut().zip(frame.pixels.iter()) {
            *a += b;
        }
    }

    let count = paths.len() as f32;
    for v in acc.pixels.iter_mut() {
        *v /= count;
    }

    Ok(Some(acc))
}

impl CalibrationMasters {
    /// Create CalibrationMasters from pre-built CFA images.
    ///
    /// Generates hot pixel map from the CFA dark if provided.
    pub fn new(dark: Option<CfaImage>, flat: Option<CfaImage>, bias: Option<CfaImage>) -> Self {
        let hot_pixel_map = dark
            .as_ref()
            .map(|d| HotPixelMap::from_master_dark(d, DEFAULT_HOT_PIXEL_SIGMA));

        Self {
            master_dark: dark,
            master_flat: flat,
            master_bias: bias,
            hot_pixel_map,
        }
    }

    /// Create CalibrationMasters by loading and averaging raw CFA files.
    ///
    /// Each set of paths is averaged pixel-wise into a master frame.
    /// Empty slices produce `None` for that master.
    pub fn from_raw_files(
        darks: &[impl AsRef<Path>],
        flats: &[impl AsRef<Path>],
        biases: &[impl AsRef<Path>],
    ) -> Result<Self> {
        let dark = compute_cfa_mean(darks)?;
        let flat = compute_cfa_mean(flats)?;
        let bias = compute_cfa_mean(biases)?;
        Ok(Self::new(dark, flat, bias))
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

        // 2. Flat division
        if let Some(ref flat) = self.master_flat {
            image.divide_by_normalized(flat, self.master_bias.as_ref());
        }

        // 3. CFA-aware hot pixel correction
        if let Some(ref hot_map) = self.hot_pixel_map {
            hot_map.correct(image);
        }
    }
}
