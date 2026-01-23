//! Calibration master frame creation and management.

use std::fs;
use std::path::Path;

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
}

impl CalibrationMasters {
    /// Create calibration masters from a directory containing Darks, Flats, and Bias subdirectories.
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

        let master_dark = Self::create_master(dir, "Darks", FrameType::Dark, method);
        let master_flat = Self::create_master(dir, "Flats", FrameType::Flat, method);
        let master_bias = Self::create_master(dir, "Bias", FrameType::Bias, method);

        Ok(Self {
            master_dark,
            master_flat,
            master_bias,
        })
    }

    /// Save all master frames to the specified directory as f32 TIFF files.
    ///
    /// Files are saved as:
    /// - master_dark.tiff
    /// - master_flat.tiff
    /// - master_bias.tiff
    pub fn save_to_directory<P: AsRef<Path>>(&self, dir: P) -> Result<()> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;

        if let Some(ref dark) = self.master_dark {
            let path = dir.join("master_dark.tiff");
            Self::save_as_tiff(dark.clone(), &path)?;
        }

        if let Some(ref flat) = self.master_flat {
            let path = dir.join("master_flat.tiff");
            Self::save_as_tiff(flat.clone(), &path)?;
        }

        if let Some(ref bias) = self.master_bias {
            let path = dir.join("master_bias.tiff");
            Self::save_as_tiff(bias.clone(), &path)?;
        }

        Ok(())
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
        use crate::test_utils::calibration_dir;

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
        if masters.master_dark.is_some() {
            assert!(output_dir.join("master_dark.tiff").exists());
        }
        if masters.master_flat.is_some() {
            assert!(output_dir.join("master_flat.tiff").exists());
        }
        if masters.master_bias.is_some() {
            assert!(output_dir.join("master_bias.tiff").exists());
        }
    }
}
