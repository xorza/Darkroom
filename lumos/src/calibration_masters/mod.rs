//! Calibration master frame creation and management.

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;

use crate::stacking::hot_pixels::HotPixelMap;
use crate::stacking::{ProgressCallback, StackConfig, stack_with_progress};
use crate::{AstroImage, FrameType};

/// Default sigma threshold for hot pixel detection.
const DEFAULT_HOT_PIXEL_SIGMA: f32 = 5.0;

/// Holds master calibration frames (dark, flat, bias) and hot pixel map.
#[derive(Debug)]
pub struct CalibrationMasters {
    /// Master dark frame
    pub master_dark: Option<AstroImage>,
    /// Master flat frame
    pub master_flat: Option<AstroImage>,
    /// Master bias frame
    pub master_bias: Option<AstroImage>,
    /// Hot pixel map derived from master dark
    pub hot_pixel_map: Option<HotPixelMap>,
    /// Stacking configuration used to create the masters
    pub config: StackConfig,
}

impl CalibrationMasters {
    /// Calibrate a light frame in place using these calibration masters.
    ///
    /// Applies the standard calibration formula:
    /// ```text
    /// calibrated = (Light - Dark) / normalize(Flat - Bias)
    /// ```
    ///
    /// 1. Subtract master dark (removes bias + thermal noise in one step)
    ///    OR subtract master bias only (if no dark available)
    /// 2. Divide by bias-corrected normalized flat: `(F - O) / mean(F - O)`
    /// 3. Correct hot pixels (replace with median of neighbors)
    ///
    /// The master dark is stored raw (not bias-subtracted), so it contains both
    /// bias and thermal signal: `dark = bias + thermal`. Subtracting it from the
    /// light removes both: `light - dark = signal`. No separate bias subtraction
    /// is needed when a dark is available.
    ///
    /// Bias is subtracted from the flat before normalization to ensure perfect
    /// vignetting cancellation. Without this, the bias offset in the flat prevents
    /// the vignetting term from canceling cleanly.
    ///
    /// # Panics
    /// Panics if the provided master frames have different dimensions than the image.
    pub fn calibrate(&self, image: &mut AstroImage) {
        if let Some(ref dark) = self.master_dark {
            // Dark contains bias + thermal. Subtracting it removes both at once:
            // light_raw - dark_raw = (signal + bias + thermal) - (bias + thermal) = signal
            assert!(
                dark.dimensions() == image.dimensions(),
                "Dark frame dimensions {:?} don't match light frame {:?}",
                dark.dimensions(),
                image.dimensions()
            );
            *image -= dark;
        } else if let Some(ref bias) = self.master_bias {
            // No dark available — subtract bias only (removes readout noise)
            assert!(
                bias.dimensions() == image.dimensions(),
                "Bias frame dimensions {:?} don't match light frame {:?}",
                bias.dimensions(),
                image.dimensions()
            );
            *image -= bias;
        }

        // Divide by normalized master flat (corrects vignetting).
        // Bias must be subtracted from flat before normalization:
        //   (F - O) / mean(F - O)
        // Without this, the bias offset prevents perfect vignetting cancellation.
        if let Some(ref flat) = self.master_flat {
            assert!(
                flat.dimensions() == image.dimensions(),
                "Flat frame dimensions {:?} don't match light frame {:?}",
                flat.dimensions(),
                image.dimensions()
            );

            // Compute mean of (flat - bias) for normalization
            let flat_mean = if let Some(ref bias) = self.master_bias {
                assert!(
                    bias.dimensions() == flat.dimensions(),
                    "Bias dimensions {:?} don't match flat dimensions {:?}",
                    bias.dimensions(),
                    flat.dimensions()
                );
                // mean(F - O) = mean(F) - mean(O)
                flat.mean() - bias.mean()
            } else {
                flat.mean()
            };
            assert!(
                flat_mean > f32::EPSILON,
                "Flat frame mean is zero or negative after bias subtraction"
            );
            let inv_flat_mean = 1.0 / flat_mean;

            let w = image.dimensions().width;
            match &self.master_bias {
                Some(bias) => {
                    // Divide by (flat - bias) / mean(flat - bias)
                    for c in 0..image.channels() {
                        let flat_ch = flat.channel(c).pixels();
                        let bias_ch = bias.channel(c).pixels();
                        let img_ch = image.channel_mut(c).pixels_mut();
                        img_ch
                            .par_chunks_mut(w)
                            .enumerate()
                            .for_each(|(row, img_row)| {
                                let off = row * w;
                                let flat_row = &flat_ch[off..off + w];
                                let bias_row = &bias_ch[off..off + w];
                                for i in 0..w {
                                    let corrected = (flat_row[i] - bias_row[i]) * inv_flat_mean;
                                    if corrected > f32::EPSILON {
                                        img_row[i] /= corrected;
                                    }
                                }
                            });
                    }
                }
                None => {
                    // No bias — divide by flat / mean(flat)
                    for c in 0..image.channels() {
                        let flat_ch = flat.channel(c).pixels();
                        let img_ch = image.channel_mut(c).pixels_mut();
                        img_ch
                            .par_chunks_mut(w)
                            .zip(flat_ch.par_chunks(w))
                            .for_each(|(img_row, flat_row)| {
                                for (d, s) in img_row.iter_mut().zip(flat_row.iter()) {
                                    let normalized_flat = s * inv_flat_mean;
                                    if normalized_flat > f32::EPSILON {
                                        *d /= normalized_flat;
                                    }
                                }
                            });
                    }
                }
            }
        }

        // Correct hot pixels (replace with median of neighbors)
        if let Some(ref hot_pixel_map) = self.hot_pixel_map {
            assert!(
                hot_pixel_map.dimensions == image.dimensions(),
                "Hot pixel map dimensions {:?} don't match light frame {:?}",
                hot_pixel_map.dimensions,
                image.dimensions()
            );
            hot_pixel_map.correct(image);
        }
    }

    /// Generate the filename for a master frame.
    fn master_filename(frame_type: FrameType, config: &StackConfig) -> String {
        let method_name = match config.method {
            crate::stacking::CombineMethod::Mean => "mean",
            crate::stacking::CombineMethod::Median => "median",
            crate::stacking::CombineMethod::WeightedMean => "weighted",
        };
        format!("master_{}_{}.tiff", frame_type, method_name)
    }

    /// Generate the full path for a master frame in a directory.
    pub(crate) fn master_path<P: AsRef<Path>>(
        dir: P,
        frame_type: FrameType,
        config: &StackConfig,
    ) -> PathBuf {
        dir.as_ref().join(Self::master_filename(frame_type, config))
    }

    /// Load existing master frames from a directory.
    ///
    /// Looks for files named master_dark_<method>.tiff, master_flat_<method>.tiff, etc.
    /// Returns None for any masters that don't exist.
    ///
    /// Master dark is stored raw (not bias-subtracted). Bias subtraction happens
    /// at calibration time in `calibrate()`.
    /// Automatically generates hot pixel map from master dark if available.
    ///
    /// # Example
    /// ```rust,ignore
    /// use lumos::{CalibrationMasters, StackConfig};
    ///
    /// // Load pre-created master frames
    /// let masters = CalibrationMasters::load("./calibration", StackConfig::default())?;
    ///
    /// // Apply calibration to a light frame
    /// let mut light = AstroImage::from_file("light_001.fits")?;
    /// masters.calibrate(&mut light);
    /// ```
    pub fn load<P: AsRef<Path>>(dir: P, config: StackConfig) -> Result<Self> {
        let dir = dir.as_ref();

        let master_dark = Self::load_master(dir, FrameType::Dark, &config);
        let master_flat = Self::load_master(dir, FrameType::Flat, &config);
        let master_bias = Self::load_master(dir, FrameType::Bias, &config);

        // Generate hot pixel map from raw master dark (before any bias subtraction)
        let hot_pixel_map = master_dark
            .as_ref()
            .map(|dark| HotPixelMap::from_master_dark(dark, DEFAULT_HOT_PIXEL_SIGMA));

        Ok(Self {
            master_dark,
            master_flat,
            master_bias,
            hot_pixel_map,
            config,
        })
    }

    /// Create calibration masters from a directory containing Darks, Flats, and Bias subdirectories.
    ///
    /// First tries to load existing master files from the directory. If not found,
    /// creates new masters by stacking raw frames from subdirectories.
    /// Automatically generates hot pixel map from master dark if available.
    ///
    /// Expected directory structure:
    /// ```text
    /// calibration_dir/
    ///   Darks/
    ///     dark1.raf, dark2.raf, ...
    ///   Flats/
    ///     flat1.raf, flat2.raf, ...
    ///   Bias/
    ///     bias1.raf, bias2.raf, ...
    /// ```
    ///
    /// Missing subdirectories are skipped (the corresponding master will be None).
    ///
    /// # Example
    /// ```rust,ignore
    /// use lumos::{CalibrationMasters, StackConfig, stacking::ProgressCallback};
    ///
    /// // Create masters from raw calibration frames
    /// let masters = CalibrationMasters::create(
    ///     "./calibration",
    ///     StackConfig::sigma_clipped(2.5),
    ///     ProgressCallback::default(),
    /// )?;
    ///
    /// // Save for future use
    /// masters.save_to_directory("./calibration")?;
    ///
    /// // Apply to light frames
    /// for path in light_frame_paths {
    ///     let mut light = AstroImage::from_file(&path)?;
    ///     masters.calibrate(&mut light);
    ///     // ... process calibrated frame ...
    /// }
    /// ```
    pub fn create<P: AsRef<Path>>(
        dir: P,
        config: StackConfig,
        progress: ProgressCallback,
    ) -> Result<Self> {
        let dir = dir.as_ref();
        assert!(
            dir.exists(),
            "Calibration directory does not exist: {:?}",
            dir
        );
        assert!(dir.is_dir(), "Path is not a directory: {:?}", dir);

        let master_bias = Self::load_master(dir, FrameType::Bias, &config)
            .or_else(|| Self::create_master(dir, "Bias", FrameType::Bias, &config, &progress));

        // Master dark is stored raw (not bias-subtracted). It contains bias + thermal.
        // calibrate() subtracts the raw dark from lights, removing both bias and thermal
        // in one step: light - dark = signal.
        let master_dark = Self::load_master(dir, FrameType::Dark, &config)
            .or_else(|| Self::create_master(dir, "Darks", FrameType::Dark, &config, &progress));

        let master_flat = Self::load_master(dir, FrameType::Flat, &config)
            .or_else(|| Self::create_master(dir, "Flats", FrameType::Flat, &config, &progress));

        // Generate hot pixel map from raw master dark (full signal range, no near-zero issues)
        let hot_pixel_map = master_dark
            .as_ref()
            .map(|dark| HotPixelMap::from_master_dark(dark, DEFAULT_HOT_PIXEL_SIGMA));

        Ok(Self {
            master_dark,
            master_flat,
            master_bias,
            hot_pixel_map,
            config,
        })
    }

    /// Save all master frames to the specified directory as f32 TIFF files.
    ///
    /// Files are saved as (e.g., for median stacking):
    /// - master_dark_median.tiff
    /// - master_flat_median.tiff
    /// - master_bias_median.tiff
    pub fn save_to_directory<P: AsRef<Path>>(&self, dir: P) -> Result<()> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;

        if let Some(ref dark) = self.master_dark {
            let path = Self::master_path(dir, FrameType::Dark, &self.config);
            Self::save_as_tiff(dark.clone(), &path)?;
        }

        if let Some(ref flat) = self.master_flat {
            let path = Self::master_path(dir, FrameType::Flat, &self.config);
            Self::save_as_tiff(flat.clone(), &path)?;
        }

        if let Some(ref bias) = self.master_bias {
            let path = Self::master_path(dir, FrameType::Bias, &self.config);
            Self::save_as_tiff(bias.clone(), &path)?;
        }

        Ok(())
    }

    fn load_master(dir: &Path, frame_type: FrameType, config: &StackConfig) -> Option<AstroImage> {
        let path = Self::master_path(dir, frame_type, config);

        if !path.exists() {
            return None;
        }

        let image = imaginarium::Image::read_file(&path).ok()?;
        Some(image.into())
    }

    fn create_master(
        base_dir: &Path,
        subdir: &str,
        frame_type: FrameType,
        config: &StackConfig,
        progress: &ProgressCallback,
    ) -> Option<AstroImage> {
        let dir = base_dir.join(subdir);
        if !dir.exists() {
            return None;
        }

        let paths = common::file_utils::astro_image_files(&dir);
        if paths.is_empty() {
            return None;
        }

        match stack_with_progress(&paths, frame_type, config.clone(), progress.clone()) {
            Ok(image) => Some(image),
            Err(e) => {
                tracing::error!("Failed to stack {:?} frames: {}", frame_type, e);
                None
            }
        }
    }

    fn save_as_tiff(image: AstroImage, path: &Path) -> Result<()> {
        let img: imaginarium::Image = image.into();
        img.save_file(path)?;
        Ok(())
    }
}
