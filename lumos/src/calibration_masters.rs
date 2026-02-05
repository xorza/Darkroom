//! Calibration master frame creation and management.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;

use crate::astro_image::HotPixelMap;
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
    /// 1. Subtract master bias (removes readout noise)
    /// 2. Subtract master dark (removes thermal noise)
    /// 3. Divide by normalized master flat (corrects vignetting and dust)
    /// 4. Correct hot pixels (replace with median of neighbors)
    ///
    /// Note: If using a dark frame that was NOT bias-subtracted, skip the separate
    /// bias subtraction as the dark already includes the bias signal.
    ///
    /// # Panics
    /// Panics if the provided master frames have different dimensions than the image.
    pub fn calibrate(&self, image: &mut AstroImage) {
        // Chunk size for parallel processing (16KB of f32s to avoid false sharing)
        const CHUNK_SIZE: usize = 4096;

        // Subtract master bias (removes readout noise)
        if let Some(ref bias) = self.master_bias {
            assert!(
                bias.dimensions() == image.dimensions(),
                "Bias frame dimensions {:?} don't match light frame {:?}",
                bias.dimensions(),
                image.dimensions()
            );
            image.apply_from_channel(bias, |_c, dst, src| {
                dst.par_chunks_mut(CHUNK_SIZE)
                    .zip(src.par_chunks(CHUNK_SIZE))
                    .for_each(|(d_chunk, s_chunk)| {
                        for (d, s) in d_chunk.iter_mut().zip(s_chunk.iter()) {
                            *d -= s;
                        }
                    });
            });
        }

        // Subtract master dark (removes thermal noise)
        if let Some(ref dark) = self.master_dark {
            assert!(
                dark.dimensions() == image.dimensions(),
                "Dark frame dimensions {:?} don't match light frame {:?}",
                dark.dimensions(),
                image.dimensions()
            );
            image.apply_from_channel(dark, |_c, dst, src| {
                dst.par_chunks_mut(CHUNK_SIZE)
                    .zip(src.par_chunks(CHUNK_SIZE))
                    .for_each(|(d_chunk, s_chunk)| {
                        for (d, s) in d_chunk.iter_mut().zip(s_chunk.iter()) {
                            *d -= s;
                        }
                    });
            });
        }

        // Divide by normalized master flat (corrects vignetting)
        if let Some(ref flat) = self.master_flat {
            assert!(
                flat.dimensions() == image.dimensions(),
                "Flat frame dimensions {:?} don't match light frame {:?}",
                flat.dimensions(),
                image.dimensions()
            );
            let flat_mean = flat.mean();
            assert!(
                flat_mean > f32::EPSILON,
                "Flat frame mean is zero or negative"
            );
            let inv_flat_mean = 1.0 / flat_mean;

            image.apply_from_channel(flat, |_c, dst, src| {
                dst.par_chunks_mut(CHUNK_SIZE)
                    .zip(src.par_chunks(CHUNK_SIZE))
                    .for_each(|(d_chunk, s_chunk)| {
                        for (d, s) in d_chunk.iter_mut().zip(s_chunk.iter()) {
                            let normalized_flat = s * inv_flat_mean;
                            if normalized_flat > f32::EPSILON {
                                *d /= normalized_flat;
                            }
                        }
                    });
            });
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
    fn master_path<P: AsRef<Path>>(dir: P, frame_type: FrameType, config: &StackConfig) -> PathBuf {
        dir.as_ref().join(Self::master_filename(frame_type, config))
    }

    /// Load existing master frames from a directory.
    ///
    /// Looks for files named master_dark_<method>.tiff, master_flat_<method>.tiff, etc.
    /// Returns None for any masters that don't exist.
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

        // Generate hot pixel map from master dark
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

        // Try to load existing masters first, then create from raw frames if not found
        let master_dark = Self::load_master(dir, FrameType::Dark, &config)
            .or_else(|| Self::create_master(dir, "Darks", FrameType::Dark, &config, &progress));
        let master_flat = Self::load_master(dir, FrameType::Flat, &config)
            .or_else(|| Self::create_master(dir, "Flats", FrameType::Flat, &config, &progress));
        let master_bias = Self::load_master(dir, FrameType::Bias, &config)
            .or_else(|| Self::create_master(dir, "Bias", FrameType::Bias, &config, &progress));

        // Generate hot pixel map from master dark
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(not(feature = "real-data"), ignore)]
    fn test_calibration_masters_from_env() {
        use crate::testing::calibration_dir;

        let Some(cal_dir) = calibration_dir() else {
            return;
        };

        let masters = CalibrationMasters::create(
            &cal_dir,
            StackConfig::default(),
            ProgressCallback::default(),
        )
        .unwrap();

        // At least one master should be created
        assert!(
            masters.master_dark.is_some()
                || masters.master_flat.is_some()
                || masters.master_bias.is_some(),
            "No master frames created"
        );

        // Save to test output
        let output_dir = common::test_utils::test_output_path("calibration_masters");
        masters.save_to_directory(&output_dir).unwrap();

        // Verify files were created
        let config = StackConfig::default();
        if masters.master_dark.is_some() {
            assert!(
                CalibrationMasters::master_path(&output_dir, FrameType::Dark, &config).exists()
            );
        }
        if masters.master_flat.is_some() {
            assert!(
                CalibrationMasters::master_path(&output_dir, FrameType::Flat, &config).exists()
            );
        }
        if masters.master_bias.is_some() {
            assert!(
                CalibrationMasters::master_path(&output_dir, FrameType::Bias, &config).exists()
            );
        }

        // Test loading saved masters
        let loaded = CalibrationMasters::load(&output_dir, StackConfig::default()).unwrap();
        assert_eq!(masters.master_dark.is_some(), loaded.master_dark.is_some());
        assert_eq!(masters.master_flat.is_some(), loaded.master_flat.is_some());
        assert_eq!(masters.master_bias.is_some(), loaded.master_bias.is_some());
    }

    #[test]
    #[cfg_attr(not(feature = "real-data"), ignore)]
    fn test_load_masters_from_calibration_masters_subdir() {
        use crate::testing::calibration_masters_dir;

        let Some(masters_dir) = calibration_masters_dir() else {
            return;
        };

        let config = StackConfig::default();
        let masters = CalibrationMasters::load(&masters_dir, config).unwrap();

        // Print what was found
        println!(
            "Loaded from calibration_masters: dark={}, flat={}, bias={}",
            masters.master_dark.is_some(),
            masters.master_flat.is_some(),
            masters.master_bias.is_some()
        );

        // At least one master should exist
        assert!(
            masters.master_dark.is_some()
                || masters.master_flat.is_some()
                || masters.master_bias.is_some(),
            "No master frames found in calibration_masters directory"
        );

        // Verify dimensions if dark exists
        if let Some(ref dark) = masters.master_dark {
            println!(
                "Master dark: {}x{}x{}",
                dark.width(),
                dark.height(),
                dark.channels()
            );
            assert!(dark.width() > 0);
            assert!(dark.height() > 0);
        }

        // Verify dimensions if flat exists
        if let Some(ref flat) = masters.master_flat {
            println!(
                "Master flat: {}x{}x{}",
                flat.width(),
                flat.height(),
                flat.channels()
            );
            assert!(flat.width() > 0);
            assert!(flat.height() > 0);
        }

        // Verify dimensions if bias exists
        if let Some(ref bias) = masters.master_bias {
            println!(
                "Master flat: {}x{}x{}",
                bias.width(),
                bias.height(),
                bias.channels()
            );
            assert!(bias.width() > 0);
            assert!(bias.height() > 0);
        }
    }
}
