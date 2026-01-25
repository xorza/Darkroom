//! Local astrometry.net source extractor using image2xy command.
//!
//! Uses the locally installed astrometry.net package's image2xy tool
//! to extract star positions from images.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// A star detected by astrometry.net's source extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstrometryStar {
    /// X coordinate in pixels (0-indexed).
    pub x: f32,
    /// Y coordinate in pixels (0-indexed).
    pub y: f32,
    /// Estimated flux (brightness).
    pub flux: f32,
    /// Estimated background at this position.
    pub background: f32,
}

/// Local source extractor using astrometry.net's image2xy command.
#[derive(Debug)]
pub struct LocalSolver {
    /// Path to image2xy executable (defaults to "image2xy").
    image2xy_path: String,
    /// Detection significance in sigmas (default 8).
    detection_sigma: f32,
    /// PSF width / Gaussian sigma (default 1 pixel).
    psf_sigma: f32,
}

impl Default for LocalSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalSolver {
    /// Create a new local solver with default settings.
    pub fn new() -> Self {
        Self {
            image2xy_path: "image2xy".to_string(),
            detection_sigma: 8.0,
            psf_sigma: 1.0,
        }
    }

    /// Set custom path to image2xy executable.
    #[allow(dead_code)]
    pub fn with_image2xy_path(mut self, path: impl Into<String>) -> Self {
        self.image2xy_path = path.into();
        self
    }

    /// Set detection significance threshold in sigmas.
    #[allow(dead_code)]
    pub fn with_detection_sigma(mut self, sigma: f32) -> Self {
        self.detection_sigma = sigma;
        self
    }

    /// Set PSF width (Gaussian sigma in pixels).
    #[allow(dead_code)]
    pub fn with_psf_sigma(mut self, sigma: f32) -> Self {
        self.psf_sigma = sigma;
        self
    }

    /// Check if image2xy is available.
    pub fn is_available() -> bool {
        Command::new("image2xy")
            .arg("-h")
            .output()
            .map(|_| true) // image2xy returns error on -h but still runs
            .unwrap_or(false)
    }

    /// Extract stars from an image and return their positions.
    ///
    /// This runs image2xy on the image and parses the resulting xylist FITS file.
    pub fn solve_and_get_stars(&self, image_path: &Path) -> Result<Vec<AstrometryStar>> {
        let image_path = image_path
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize path: {}", image_path.display()))?;

        // Create a temp directory for output files
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let output_file = temp_dir.path().join("stars.xyls");

        let mut cmd = Command::new(&self.image2xy_path);

        cmd.arg("-O") // Overwrite existing
            .arg("-o")
            .arg(&output_file)
            .arg("-p")
            .arg(self.detection_sigma.to_string())
            .arg("-w")
            .arg(self.psf_sigma.to_string())
            .arg(&image_path);

        tracing::info!("Running image2xy on {}", image_path.display());
        tracing::debug!("Command: {:?}", cmd);

        let output = cmd.output().context("Failed to execute image2xy command")?;

        // image2xy may return non-zero even on success, check if output file exists
        if !output_file.exists() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::error!("image2xy failed:\nstdout: {}\nstderr: {}", stdout, stderr);
            bail!(
                "image2xy failed - no output file created. Status: {}, stderr: {}",
                output.status,
                stderr
            );
        }

        Self::parse_xyls_file(&output_file)
    }

    /// Parse an xylist FITS file from disk.
    fn parse_xyls_file(path: &Path) -> Result<Vec<AstrometryStar>> {
        use fitsio::FitsFile;

        let mut fptr = FitsFile::open(path).context("Failed to open xylist FITS file")?;

        // The xylist file has a BINTABLE extension with star data
        let hdu = fptr.hdu(1).context("No extension HDU in xylist file")?;

        // Read column data - image2xy uses uppercase column names
        let x_col: Vec<f32> = hdu
            .read_col(&mut fptr, "X")
            .or_else(|_| hdu.read_col(&mut fptr, "x"))
            .context("Failed to read X column")?;

        let y_col: Vec<f32> = hdu
            .read_col(&mut fptr, "Y")
            .or_else(|_| hdu.read_col(&mut fptr, "y"))
            .context("Failed to read Y column")?;

        // Flux is usually available
        let flux_col: Vec<f32> = hdu
            .read_col(&mut fptr, "FLUX")
            .or_else(|_| hdu.read_col(&mut fptr, "flux"))
            .unwrap_or_else(|_| vec![1.0; x_col.len()]);

        // Background may or may not be present
        let bg_col: Vec<f32> = hdu
            .read_col(&mut fptr, "BACKGROUND")
            .or_else(|_| hdu.read_col(&mut fptr, "background"))
            .unwrap_or_else(|_| vec![0.0; x_col.len()]);

        let stars: Vec<AstrometryStar> = x_col
            .into_iter()
            .zip(y_col)
            .zip(flux_col)
            .zip(bg_col)
            .map(|(((x, y), flux), background)| {
                AstrometryStar {
                    // Convert from 1-indexed FITS to 0-indexed
                    x: x - 1.0,
                    y: y - 1.0,
                    flux,
                    background,
                }
            })
            .collect();

        tracing::info!("Parsed {} stars from xylist file", stars.len());
        Ok(stars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_solver_available() {
        // This test will pass if image2xy is installed
        let available = LocalSolver::is_available();
        println!("image2xy available: {}", available);
    }
}
