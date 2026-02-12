//! Calibration master frame creation and management.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::Result;

use crate::astro_image::cfa::{CfaImage, CfaType};
use crate::common::Buffer2;
use crate::raw::load_raw_cfa;
use crate::stacking::FrameType;
use crate::stacking::cache::ImageCache;
use crate::stacking::config::{Normalization, StackConfig};
use crate::stacking::hot_pixels::HotPixelMap;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::stack::{compute_multiplicative_norm_params, dispatch_stacking_generic};

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

/// Stack raw CFA frames using the full stacking pipeline.
///
/// Uses median for < 8 frames, sigma-clipped mean for >= 8 frames.
/// Returns `None` if `paths` is empty.
fn stack_cfa_frames(
    paths: &[impl AsRef<Path> + Sync],
    frame_type: FrameType,
    normalization: Normalization,
) -> Result<Option<CfaImage>> {
    if paths.is_empty() {
        return Ok(None);
    }

    // Load the first frame to capture the CFA pattern (not stored in cache metadata)
    // todo load only header
    let first = load_raw_cfa(paths[0].as_ref())?;
    let pattern = first.pattern.clone();
    drop(first);

    let config = if paths.len() < 8 {
        StackConfig {
            normalization,
            ..StackConfig::median()
        }
    } else {
        StackConfig {
            normalization,
            ..StackConfig::sigma_clipped(3.0)
        }
    };

    let cache = ImageCache::<CfaImage>::from_paths(
        paths,
        &config.cache,
        frame_type,
        ProgressCallback::default(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let norm_params = match config.normalization {
        Normalization::None => None,
        Normalization::Multiplicative => Some(compute_multiplicative_norm_params(&cache)),
        Normalization::Global => Some(crate::stacking::stack::compute_global_norm_params(&cache)),
    };

    let channels = dispatch_stacking_generic(&cache, &config, paths.len(), norm_params.as_deref());

    // CfaImage has exactly 1 channel
    let pixels = channels.into_iter().next().unwrap();

    Ok(Some(CfaImage {
        data: Buffer2::new(cache.dimensions().width, cache.dimensions().height, pixels),
        pattern,
        metadata: cache.metadata().clone(),
    }))
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

    /// Create CalibrationMasters by stacking raw CFA files.
    ///
    /// Uses sigma-clipped mean (>= 8 frames) or median (< 8 frames)
    /// with the full stacking pipeline (rejection, normalization, chunked processing).
    /// Empty slices produce `None` for that master.
    pub fn from_raw_files(
        darks: &[impl AsRef<Path> + Sync],
        flats: &[impl AsRef<Path> + Sync],
        biases: &[impl AsRef<Path> + Sync],
    ) -> Result<Self> {
        let dark = stack_cfa_frames(darks, FrameType::Dark, Normalization::None)?;
        let flat = stack_cfa_frames(flats, FrameType::Flat, Normalization::Multiplicative)?;
        let bias = stack_cfa_frames(biases, FrameType::Bias, Normalization::None)?;
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
