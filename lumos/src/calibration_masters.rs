//! Calibration master frame creation and management.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{AstroImage, FrameType, StackingMethod, stack_frames};

/// Holds master calibration frames (dark, flat, bias).
#[derive(Debug)]
pub struct CalibrationMasters {
    /// Master dark frame
    pub master_dark: Option<AstroImage>,
    /// Master flat frame
    pub master_flat: Option<AstroImage>,
    /// Master bias frame
    pub master_bias: Option<AstroImage>,
    /// Stacking method used to create the masters
    pub method: StackingMethod,
}

impl CalibrationMasters {
    /// Generate the filename for a master frame.
    pub fn master_filename(frame_type: FrameType, method: StackingMethod) -> String {
        format!("master_{}_{}.tiff", frame_type, method)
    }

    /// Generate the full path for a master frame in a directory.
    pub fn master_path<P: AsRef<Path>>(
        dir: P,
        frame_type: FrameType,
        method: StackingMethod,
    ) -> PathBuf {
        dir.as_ref().join(Self::master_filename(frame_type, method))
    }

    /// Load existing master frames from a directory.
    ///
    /// Looks for files named master_dark_<method>.tiff, master_flat_<method>.tiff, etc.
    /// Returns None for any masters that don't exist.
    pub fn load_from_directory<P: AsRef<Path>>(dir: P, method: StackingMethod) -> Result<Self> {
        let dir = dir.as_ref();

        let master_dark = Self::load_master(dir, FrameType::Dark, method);
        let master_flat = Self::load_master(dir, FrameType::Flat, method);
        let master_bias = Self::load_master(dir, FrameType::Bias, method);

        Ok(Self {
            master_dark,
            master_flat,
            master_bias,
            method,
        })
    }

    /// Create calibration masters from a directory containing Darks, Flats, and Bias subdirectories.
    ///
    /// First tries to load existing master files from the directory. If not found,
    /// creates new masters by stacking raw frames from subdirectories.
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
    pub fn from_directory<P: AsRef<Path>>(dir: P, method: StackingMethod) -> Result<Self> {
        let dir = dir.as_ref();
        assert!(
            dir.exists(),
            "Calibration directory does not exist: {:?}",
            dir
        );
        assert!(dir.is_dir(), "Path is not a directory: {:?}", dir);

        // Try to load existing masters first, then create from raw frames if not found
        let master_dark = Self::load_master(dir, FrameType::Dark, method)
            .or_else(|| Self::create_master(dir, "Darks", FrameType::Dark, method));
        let master_flat = Self::load_master(dir, FrameType::Flat, method)
            .or_else(|| Self::create_master(dir, "Flats", FrameType::Flat, method));
        let master_bias = Self::load_master(dir, FrameType::Bias, method)
            .or_else(|| Self::create_master(dir, "Bias", FrameType::Bias, method));

        Ok(Self {
            master_dark,
            master_flat,
            master_bias,
            method,
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
            let path = Self::master_path(dir, FrameType::Dark, self.method);
            Self::save_as_tiff(dark.clone(), &path)?;
        }

        if let Some(ref flat) = self.master_flat {
            let path = Self::master_path(dir, FrameType::Flat, self.method);
            Self::save_as_tiff(flat.clone(), &path)?;
        }

        if let Some(ref bias) = self.master_bias {
            let path = Self::master_path(dir, FrameType::Bias, self.method);
            Self::save_as_tiff(bias.clone(), &path)?;
        }

        Ok(())
    }

    fn load_master(
        dir: &Path,
        frame_type: FrameType,
        method: StackingMethod,
    ) -> Option<AstroImage> {
        let path = Self::master_path(dir, frame_type, method);
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
        method: StackingMethod,
    ) -> Option<AstroImage> {
        let dir = base_dir.join(subdir);
        if !dir.exists() {
            return None;
        }

        let frames = AstroImage::load_from_directory(&dir);
        if frames.is_empty() {
            return None;
        }

        Some(stack_frames(&frames, method, frame_type))
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
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_calibration_masters_from_env() {
        use crate::testing::test_utils::calibration_dir_with_message as calibration_dir;

        let Some(cal_dir) = calibration_dir() else {
            return;
        };

        let masters = CalibrationMasters::from_directory(&cal_dir, StackingMethod::Median).unwrap();

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
        let method = StackingMethod::Median;
        if masters.master_dark.is_some() {
            assert!(CalibrationMasters::master_path(&output_dir, FrameType::Dark, method).exists());
        }
        if masters.master_flat.is_some() {
            assert!(CalibrationMasters::master_path(&output_dir, FrameType::Flat, method).exists());
        }
        if masters.master_bias.is_some() {
            assert!(CalibrationMasters::master_path(&output_dir, FrameType::Bias, method).exists());
        }

        // Test loading saved masters
        let loaded = CalibrationMasters::load_from_directory(&output_dir, method).unwrap();
        assert_eq!(masters.master_dark.is_some(), loaded.master_dark.is_some());
        assert_eq!(masters.master_flat.is_some(), loaded.master_flat.is_some());
        assert_eq!(masters.master_bias.is_some(), loaded.master_bias.is_some());
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_load_masters_from_calibration_masters_subdir() {
        use crate::testing::test_utils::calibration_masters_dir_with_message as calibration_masters_dir;

        let Some(masters_dir) = calibration_masters_dir() else {
            return;
        };

        let method = StackingMethod::Median;
        let masters = CalibrationMasters::load_from_directory(&masters_dir, method).unwrap();

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
                dark.dimensions.width, dark.dimensions.height, dark.dimensions.channels
            );
            assert!(dark.dimensions.width > 0);
            assert!(dark.dimensions.height > 0);
        }

        // Verify dimensions if flat exists
        if let Some(ref flat) = masters.master_flat {
            println!(
                "Master flat: {}x{}x{}",
                flat.dimensions.width, flat.dimensions.height, flat.dimensions.channels
            );
            assert!(flat.dimensions.width > 0);
            assert!(flat.dimensions.height > 0);
        }

        // Verify dimensions if bias exists
        if let Some(ref bias) = masters.master_bias {
            println!(
                "Master flat: {}x{}x{}",
                bias.dimensions.width, bias.dimensions.height, bias.dimensions.channels
            );
            assert!(bias.dimensions.width > 0);
            assert!(bias.dimensions.height > 0);
        }
    }
}
