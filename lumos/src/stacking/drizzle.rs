//! Drizzle algorithm for super-resolution image stacking.
//!
//! Implementation of variable-pixel linear reconstruction (Drizzle) algorithm
//! originally developed by Fruchter & Hook (2002) for Hubble Space Telescope.
//!
//! The algorithm shrinks input pixels into smaller "drops" and maps them onto
//! a finer output grid, preserving flux and improving resolution when combining
//! dithered images.
//!
//! # Key Parameters
//!
//! - **scale**: Output resolution multiplier (e.g., 2.0 = 2x resolution)
//! - **pixfrac**: Ratio of drop size to input pixel (0.0-1.0)
//!   - pixfrac=1.0: equivalent to shift-and-add (preserves noise correlation)
//!   - pixfrac=0.5: good balance for 4-point dithers
//!   - pixfrac→0: equivalent to interlacing (requires good dithering)
//!
//! # References
//!
//! - Fruchter & Hook (2002): "Drizzle: A Method for the Linear Reconstruction
//!   of Undersampled Images"
//! - HST DrizzlePac Handbook (2025)

// Allow dead code for now - this is a new module with public API
#![allow(dead_code)]

use std::path::Path;

use crate::astro_image::AstroImage;
use crate::registration::types::TransformMatrix;
use crate::stacking::error::Error;
use crate::stacking::progress::{ProgressCallback, StackingStage, report_progress};

/// Drizzle kernel type for distributing flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrizzleKernel {
    /// Square droplet aligned with input pixel.
    /// Most accurate, preserves flux exactly. Default.
    #[default]
    Square,
    /// Point kernel - single pixel contribution.
    /// Fastest but requires very good dithering.
    Point,
    /// Gaussian droplet with configurable FWHM.
    /// Smoother output, slight flux redistribution.
    Gaussian,
    /// Lanczos kernel for high-quality interpolation.
    /// Best quality but slowest.
    Lanczos,
}

/// Configuration for Drizzle stacking.
#[derive(Debug, Clone)]
pub struct DrizzleConfig {
    /// Output scale factor relative to input (e.g., 2.0 = 2x resolution).
    /// Common values: 1.5, 2.0, 3.0
    pub scale: f32,
    /// Pixel fraction - ratio of drop size to input pixel before mapping.
    /// Range: 0.0 to 1.0
    /// - 1.0 = shift-and-add (full pixel footprint)
    /// - 0.8 = recommended for 4-point dithered data
    /// - 0.5 = aggressive shrinking, needs good dithering
    pub pixfrac: f32,
    /// Kernel type for flux distribution.
    pub kernel: DrizzleKernel,
    /// Fill value for pixels with no coverage.
    pub fill_value: f32,
    /// Minimum coverage threshold (0.0-1.0).
    /// Pixels with coverage below this are set to fill_value.
    pub min_coverage: f32,
}

impl Default for DrizzleConfig {
    fn default() -> Self {
        Self {
            scale: 2.0,
            pixfrac: 0.8,
            kernel: DrizzleKernel::Square,
            fill_value: 0.0,
            min_coverage: 0.1,
        }
    }
}

impl DrizzleConfig {
    /// Create config for 2x super-resolution with default parameters.
    pub fn x2() -> Self {
        Self::default()
    }

    /// Create config for 1.5x super-resolution.
    pub fn x1_5() -> Self {
        Self {
            scale: 1.5,
            ..Default::default()
        }
    }

    /// Create config for 3x super-resolution.
    pub fn x3() -> Self {
        Self {
            scale: 3.0,
            pixfrac: 0.7, // Slightly smaller drops for higher resolution
            ..Default::default()
        }
    }

    /// Set pixel fraction.
    pub fn with_pixfrac(mut self, pixfrac: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&pixfrac),
            "pixfrac must be between 0.0 and 1.0"
        );
        self.pixfrac = pixfrac;
        self
    }

    /// Set kernel type.
    pub fn with_kernel(mut self, kernel: DrizzleKernel) -> Self {
        self.kernel = kernel;
        self
    }

    /// Set minimum coverage threshold.
    pub fn with_min_coverage(mut self, min_coverage: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&min_coverage),
            "min_coverage must be between 0.0 and 1.0"
        );
        self.min_coverage = min_coverage;
        self
    }
}

/// Drizzle accumulator for building the output image.
#[derive(Debug)]
pub struct DrizzleAccumulator {
    /// Accumulated weighted flux values.
    data: Vec<f32>,
    /// Accumulated weights (coverage map).
    weights: Vec<f32>,
    /// Output image width.
    width: usize,
    /// Output image height.
    height: usize,
    /// Number of channels.
    channels: usize,
    /// Configuration.
    config: DrizzleConfig,
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for the given input dimensions.
    pub fn new(
        input_width: usize,
        input_height: usize,
        channels: usize,
        config: DrizzleConfig,
    ) -> Self {
        let output_width = (input_width as f32 * config.scale).ceil() as usize;
        let output_height = (input_height as f32 * config.scale).ceil() as usize;
        let total_pixels = output_width * output_height * channels;

        Self {
            data: vec![0.0; total_pixels],
            weights: vec![0.0; total_pixels],
            width: output_width,
            height: output_height,
            channels,
            config,
        }
    }

    /// Output dimensions.
    pub fn dimensions(&self) -> (usize, usize, usize) {
        (self.width, self.height, self.channels)
    }

    /// Add an image to the drizzle accumulator with the given transform.
    ///
    /// The transform maps input pixel coordinates to reference (output) coordinates.
    /// It should include any registration alignment computed from star matching.
    pub fn add_image(&mut self, image: &AstroImage, transform: &TransformMatrix, weight: f32) {
        let input_width = image.width();
        let input_height = image.height();
        let input_channels = image.channels();

        assert_eq!(
            input_channels, self.channels,
            "Channel count mismatch: expected {}, got {}",
            self.channels, input_channels
        );

        let scale = self.config.scale;
        let pixfrac = self.config.pixfrac;
        let drop_size = pixfrac / scale; // Drop size in output pixels

        let pixels = image.pixels();

        match self.config.kernel {
            DrizzleKernel::Square => {
                self.add_image_square(
                    pixels,
                    transform,
                    weight,
                    input_width,
                    input_height,
                    scale,
                    drop_size,
                );
            }
            DrizzleKernel::Point => {
                self.add_image_point(pixels, transform, weight, input_width, input_height, scale);
            }
            DrizzleKernel::Gaussian => {
                self.add_image_gaussian(
                    pixels,
                    transform,
                    weight,
                    input_width,
                    input_height,
                    scale,
                    drop_size,
                );
            }
            DrizzleKernel::Lanczos => {
                self.add_image_lanczos(
                    pixels,
                    transform,
                    weight,
                    input_width,
                    input_height,
                    scale,
                    drop_size,
                );
            }
        }
    }

    /// Add image using square kernel (most accurate).
    #[allow(clippy::too_many_arguments)]
    fn add_image_square(
        &mut self,
        pixels: &[f32],
        transform: &TransformMatrix,
        weight: f32,
        input_width: usize,
        input_height: usize,
        scale: f32,
        drop_size: f32,
    ) {
        let half_drop = drop_size / 2.0;
        let inv_area = 1.0 / (drop_size * drop_size);
        let output_width = self.width;
        let output_height = self.height;
        let channels = self.channels;

        // Process each input pixel
        for iy in 0..input_height {
            for ix in 0..input_width {
                // Transform input pixel center to output coordinates
                let (ox_center, oy_center) =
                    transform_point(transform, ix as f32 + 0.5, iy as f32 + 0.5);

                // Scale to output grid
                let ox_center = ox_center * scale;
                let oy_center = oy_center * scale;

                // Compute drop bounding box in output pixels
                let ox_min = (ox_center - half_drop).floor().max(0.0) as usize;
                let oy_min = (oy_center - half_drop).floor().max(0.0) as usize;
                let ox_max = (ox_center + half_drop).ceil().min(output_width as f32) as usize;
                let oy_max = (oy_center + half_drop).ceil().min(output_height as f32) as usize;

                // Get input pixel values
                let input_idx_base = (iy * input_width + ix) * channels;

                // Distribute flux to overlapping output pixels
                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
                        // Compute overlap area between drop and output pixel
                        let overlap = compute_square_overlap(
                            ox_center - half_drop,
                            oy_center - half_drop,
                            ox_center + half_drop,
                            oy_center + half_drop,
                            ox as f32,
                            oy as f32,
                            (ox + 1) as f32,
                            (oy + 1) as f32,
                        );

                        if overlap > 0.0 {
                            // Weight by overlap area normalized by drop area
                            let pixel_weight = weight * overlap * inv_area;
                            let output_idx_base = (oy * output_width + ox) * channels;

                            for c in 0..channels {
                                let flux = pixels[input_idx_base + c];
                                self.data[output_idx_base + c] += flux * pixel_weight;
                                self.weights[output_idx_base + c] += pixel_weight;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Add image using point kernel (fastest, needs good dithering).
    fn add_image_point(
        &mut self,
        pixels: &[f32],
        transform: &TransformMatrix,
        weight: f32,
        input_width: usize,
        input_height: usize,
        scale: f32,
    ) {
        let output_width = self.width;
        let output_height = self.height;
        let channels = self.channels;

        for iy in 0..input_height {
            for ix in 0..input_width {
                // Transform and scale to output coordinates
                let (ox, oy) = transform_point(transform, ix as f32 + 0.5, iy as f32 + 0.5);
                let ox = (ox * scale).floor() as isize;
                let oy = (oy * scale).floor() as isize;

                if ox >= 0 && ox < output_width as isize && oy >= 0 && oy < output_height as isize {
                    let ox = ox as usize;
                    let oy = oy as usize;
                    let input_idx_base = (iy * input_width + ix) * channels;
                    let output_idx_base = (oy * output_width + ox) * channels;

                    for c in 0..channels {
                        let flux = pixels[input_idx_base + c];
                        self.data[output_idx_base + c] += flux * weight;
                        self.weights[output_idx_base + c] += weight;
                    }
                }
            }
        }
    }

    /// Add image using Gaussian kernel (smoother output).
    #[allow(clippy::too_many_arguments)]
    fn add_image_gaussian(
        &mut self,
        pixels: &[f32],
        transform: &TransformMatrix,
        weight: f32,
        input_width: usize,
        input_height: usize,
        scale: f32,
        drop_size: f32,
    ) {
        let sigma = drop_size / 2.355; // FWHM to sigma
        let radius = (3.0 * sigma).ceil() as isize;
        let inv_2sigma_sq = 1.0 / (2.0 * sigma * sigma);
        let output_width = self.width;
        let output_height = self.height;
        let channels = self.channels;

        for iy in 0..input_height {
            for ix in 0..input_width {
                let (ox_center, oy_center) =
                    transform_point(transform, ix as f32 + 0.5, iy as f32 + 0.5);
                let ox_center = ox_center * scale;
                let oy_center = oy_center * scale;

                let ox_int = ox_center.floor() as isize;
                let oy_int = oy_center.floor() as isize;

                let input_idx_base = (iy * input_width + ix) * channels;

                // Accumulate Gaussian weights for normalization
                let mut total_gauss_weight = 0.0f32;

                // First pass: compute total weight
                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height as isize {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width as isize {
                            continue;
                        }

                        let dist_x = (ox as f32 + 0.5) - ox_center;
                        let dist_y = (oy as f32 + 0.5) - oy_center;
                        let dist_sq = dist_x * dist_x + dist_y * dist_y;
                        let gauss_weight = (-dist_sq * inv_2sigma_sq).exp();
                        total_gauss_weight += gauss_weight;
                    }
                }

                if total_gauss_weight < 1e-10 {
                    continue;
                }

                // Second pass: distribute flux
                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height as isize {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width as isize {
                            continue;
                        }

                        let ox = ox as usize;
                        let oy = oy as usize;

                        let dist_x = (ox as f32 + 0.5) - ox_center;
                        let dist_y = (oy as f32 + 0.5) - oy_center;
                        let dist_sq = dist_x * dist_x + dist_y * dist_y;
                        let gauss_weight = (-dist_sq * inv_2sigma_sq).exp();

                        let pixel_weight = weight * gauss_weight / total_gauss_weight;
                        let output_idx_base = (oy * output_width + ox) * channels;

                        for c in 0..channels {
                            let flux = pixels[input_idx_base + c];
                            self.data[output_idx_base + c] += flux * pixel_weight;
                            self.weights[output_idx_base + c] += pixel_weight;
                        }
                    }
                }
            }
        }
    }

    /// Add image using Lanczos kernel (highest quality).
    #[allow(clippy::too_many_arguments)]
    fn add_image_lanczos(
        &mut self,
        pixels: &[f32],
        transform: &TransformMatrix,
        weight: f32,
        input_width: usize,
        input_height: usize,
        scale: f32,
        drop_size: f32,
    ) {
        // Use Lanczos-3 (support radius 3)
        let a = 3.0f32;
        let radius = (a * drop_size / scale).max(a).ceil() as isize;
        let output_width = self.width;
        let output_height = self.height;
        let channels = self.channels;

        for iy in 0..input_height {
            for ix in 0..input_width {
                let (ox_center, oy_center) =
                    transform_point(transform, ix as f32 + 0.5, iy as f32 + 0.5);
                let ox_center = ox_center * scale;
                let oy_center = oy_center * scale;

                let ox_int = ox_center.floor() as isize;
                let oy_int = oy_center.floor() as isize;

                let input_idx_base = (iy * input_width + ix) * channels;

                // Compute Lanczos weights and normalize
                let mut total_lanczos_weight = 0.0f32;

                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height as isize {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width as isize {
                            continue;
                        }

                        let dist_x = (ox_center - (ox as f32 + 0.5)) / drop_size;
                        let dist_y = (oy_center - (oy as f32 + 0.5)) / drop_size;
                        let lanczos_weight = lanczos_kernel(dist_x, a) * lanczos_kernel(dist_y, a);
                        total_lanczos_weight += lanczos_weight;
                    }
                }

                if total_lanczos_weight.abs() < 1e-10 {
                    continue;
                }

                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height as isize {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width as isize {
                            continue;
                        }

                        let ox = ox as usize;
                        let oy = oy as usize;

                        let dist_x = (ox_center - (ox as f32 + 0.5)) / drop_size;
                        let dist_y = (oy_center - (oy as f32 + 0.5)) / drop_size;
                        let lanczos_weight = lanczos_kernel(dist_x, a) * lanczos_kernel(dist_y, a);

                        let pixel_weight = weight * lanczos_weight / total_lanczos_weight;
                        let output_idx_base = (oy * output_width + ox) * channels;

                        for c in 0..channels {
                            let flux = pixels[input_idx_base + c];
                            self.data[output_idx_base + c] += flux * pixel_weight;
                            self.weights[output_idx_base + c] += pixel_weight;
                        }
                    }
                }
            }
        }
    }

    /// Finalize the drizzle result, normalizing by coverage weights.
    pub fn finalize(self) -> DrizzleResult {
        let total_pixels = self.width * self.height * self.channels;
        let mut output_data = vec![0.0f32; total_pixels];
        let mut coverage = vec![0.0f32; self.width * self.height];

        let min_coverage = self.config.min_coverage;
        let fill_value = self.config.fill_value;

        for y in 0..self.height {
            for x in 0..self.width {
                let pixel_idx = y * self.width + x;

                // Compute coverage as average weight across channels
                let mut total_weight = 0.0f32;
                for c in 0..self.channels {
                    total_weight += self.weights[pixel_idx * self.channels + c];
                }
                let avg_weight = total_weight / self.channels as f32;
                coverage[pixel_idx] = avg_weight;

                // Normalize flux by weight or fill with fill_value
                for c in 0..self.channels {
                    let data_idx = pixel_idx * self.channels + c;
                    let weight = self.weights[data_idx];

                    if weight >= min_coverage {
                        output_data[data_idx] = self.data[data_idx] / weight;
                    } else {
                        output_data[data_idx] = fill_value;
                    }
                }
            }
        }

        // Find max coverage for normalization
        let max_coverage = coverage.iter().copied().fold(0.0f32, f32::max);
        if max_coverage > 0.0 {
            for c in &mut coverage {
                *c /= max_coverage;
            }
        }

        let image = AstroImage::from_pixels(self.width, self.height, self.channels, output_data);

        DrizzleResult { image, coverage }
    }
}

/// Result of drizzle stacking.
#[derive(Debug)]
pub struct DrizzleResult {
    /// The drizzled output image.
    pub image: AstroImage,
    /// Normalized coverage map (0.0 = no data, 1.0 = maximum coverage).
    pub coverage: Vec<f32>,
}

impl DrizzleResult {
    /// Get coverage at a specific pixel.
    pub fn coverage_at(&self, x: usize, y: usize) -> f32 {
        let idx = y * self.image.width() + x;
        self.coverage.get(idx).copied().unwrap_or(0.0)
    }
}

/// Compute overlap area between two axis-aligned rectangles.
#[inline]
#[allow(clippy::too_many_arguments)]
fn compute_square_overlap(
    ax1: f32,
    ay1: f32,
    ax2: f32,
    ay2: f32,
    bx1: f32,
    by1: f32,
    bx2: f32,
    by2: f32,
) -> f32 {
    let x_overlap = (ax2.min(bx2) - ax1.max(bx1)).max(0.0);
    let y_overlap = (ay2.min(by2) - ay1.max(by1)).max(0.0);
    x_overlap * y_overlap
}

/// Transform a point using the transformation matrix.
///
/// The matrix is stored in row-major order:
/// | a  b  c |   | data[0] data[1] data[2] |
/// | d  e  f | = | data[3] data[4] data[5] |
/// | g  h  1 |   | data[6] data[7] data[8] |
#[inline]
fn transform_point(transform: &TransformMatrix, x: f32, y: f32) -> (f32, f32) {
    let m = &transform.data;
    let x64 = x as f64;
    let y64 = y as f64;
    let w = m[6] * x64 + m[7] * y64 + m[8];
    let out_x = ((m[0] * x64 + m[1] * y64 + m[2]) / w) as f32;
    let out_y = ((m[3] * x64 + m[4] * y64 + m[5]) / w) as f32;
    (out_x, out_y)
}

/// Lanczos kernel function.
#[inline]
fn lanczos_kernel(x: f32, a: f32) -> f32 {
    if x.abs() < 1e-6 {
        return 1.0;
    }
    if x.abs() >= a {
        return 0.0;
    }
    let pi_x = std::f32::consts::PI * x;
    let pi_x_a = pi_x / a;
    (pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)
}

/// Drizzle stack images from paths with transforms.
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `transforms` - Per-image transformation matrices (maps input to reference)
/// * `weights` - Optional per-frame weights (quality-based)
/// * `config` - Drizzle configuration
/// * `progress` - Progress callback
///
/// # Returns
///
/// The drizzled result with image and coverage map.
pub fn drizzle_stack<P: AsRef<Path> + Sync>(
    paths: &[P],
    transforms: &[TransformMatrix],
    weights: Option<&[f32]>,
    config: &DrizzleConfig,
    progress: ProgressCallback,
) -> Result<DrizzleResult, Error> {
    if paths.is_empty() {
        return Err(Error::NoPaths);
    }

    assert_eq!(
        paths.len(),
        transforms.len(),
        "Number of paths ({}) must match number of transforms ({})",
        paths.len(),
        transforms.len()
    );

    if let Some(w) = weights {
        assert_eq!(
            paths.len(),
            w.len(),
            "Number of paths ({}) must match number of weights ({})",
            paths.len(),
            w.len()
        );
    }

    // Load first image to get dimensions
    let first_image = AstroImage::from_file(paths[0].as_ref()).map_err(|e| Error::ImageLoad {
        path: paths[0].as_ref().to_path_buf(),
        source: std::io::Error::other(e.to_string()),
    })?;

    let input_width = first_image.width();
    let input_height = first_image.height();
    let channels = first_image.channels();

    tracing::info!(
        input_width,
        input_height,
        channels,
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        frame_count = paths.len(),
        "Starting drizzle stacking"
    );

    let mut accumulator =
        DrizzleAccumulator::new(input_width, input_height, channels, config.clone());
    let (out_w, out_h, _) = accumulator.dimensions();
    tracing::info!(
        output_width = out_w,
        output_height = out_h,
        "Output dimensions"
    );

    // Add first image
    let first_weight = weights.map_or(1.0, |w| w[0]);
    accumulator.add_image(&first_image, &transforms[0], first_weight);
    report_progress(&progress, 1, paths.len(), StackingStage::Processing);

    // Process remaining images
    for (i, path) in paths.iter().enumerate().skip(1) {
        let image = AstroImage::from_file(path.as_ref()).map_err(|e| Error::ImageLoad {
            path: path.as_ref().to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })?;

        // Validate dimensions match
        if image.width() != input_width || image.height() != input_height {
            return Err(Error::ImageLoad {
                path: path.as_ref().to_path_buf(),
                source: std::io::Error::other(format!(
                    "Dimension mismatch: expected {}x{}, got {}x{}",
                    input_width,
                    input_height,
                    image.width(),
                    image.height()
                )),
            });
        }

        let weight = weights.map_or(1.0, |w| w[i]);
        accumulator.add_image(&image, &transforms[i], weight);
        report_progress(&progress, i + 1, paths.len(), StackingStage::Processing);
    }

    let result = accumulator.finalize();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drizzle_config_default() {
        let config = DrizzleConfig::default();
        assert!((config.scale - 2.0).abs() < f32::EPSILON);
        assert!((config.pixfrac - 0.8).abs() < f32::EPSILON);
        assert_eq!(config.kernel, DrizzleKernel::Square);
    }

    #[test]
    fn test_drizzle_config_presets() {
        let x1_5 = DrizzleConfig::x1_5();
        assert!((x1_5.scale - 1.5).abs() < f32::EPSILON);

        let x2 = DrizzleConfig::x2();
        assert!((x2.scale - 2.0).abs() < f32::EPSILON);

        let x3 = DrizzleConfig::x3();
        assert!((x3.scale - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_drizzle_config_builder() {
        let config = DrizzleConfig::default()
            .with_pixfrac(0.5)
            .with_kernel(DrizzleKernel::Gaussian)
            .with_min_coverage(0.2);

        assert!((config.pixfrac - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.kernel, DrizzleKernel::Gaussian);
        assert!((config.min_coverage - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    #[should_panic(expected = "pixfrac must be between")]
    fn test_drizzle_config_invalid_pixfrac() {
        DrizzleConfig::default().with_pixfrac(1.5);
    }

    #[test]
    fn test_drizzle_accumulator_dimensions() {
        let config = DrizzleConfig::x2();
        let acc = DrizzleAccumulator::new(100, 80, 3, config);
        let (w, h, c) = acc.dimensions();
        assert_eq!(w, 200);
        assert_eq!(h, 160);
        assert_eq!(c, 3);
    }

    #[test]
    fn test_compute_square_overlap() {
        // Full overlap (unit squares at same position)
        let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0);
        assert!((overlap - 1.0).abs() < f32::EPSILON);

        // No overlap
        let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0);
        assert!((overlap - 0.0).abs() < f32::EPSILON);

        // Partial overlap (half)
        let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 0.5, 0.0, 1.5, 1.0);
        assert!((overlap - 0.5).abs() < f32::EPSILON);

        // Quarter overlap
        let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 0.5, 0.5, 1.5, 1.5);
        assert!((overlap - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn test_lanczos_kernel() {
        // Center value
        assert!((lanczos_kernel(0.0, 3.0) - 1.0).abs() < f32::EPSILON);

        // Outside support
        assert!((lanczos_kernel(3.5, 3.0) - 0.0).abs() < f32::EPSILON);

        // Symmetry
        let pos = lanczos_kernel(1.5, 3.0);
        let neg = lanczos_kernel(-1.5, 3.0);
        assert!((pos - neg).abs() < 1e-6);
    }

    #[test]
    fn test_transform_point_identity() {
        let identity = TransformMatrix::identity();
        let (x, y) = transform_point(&identity, 10.0, 20.0);
        assert!((x - 10.0).abs() < f32::EPSILON);
        assert!((y - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_transform_point_translation() {
        let translation = TransformMatrix::translation(5.0, -3.0);
        let (x, y) = transform_point(&translation, 10.0, 20.0);
        assert!((x - 15.0).abs() < f32::EPSILON);
        assert!((y - 17.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_drizzle_single_image() {
        // Create a simple test image
        let image = AstroImage::from_pixels(100, 100, 1, vec![0.5; 100 * 100]);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(100, 100, 1, config);

        let identity = TransformMatrix::identity();
        acc.add_image(&image, &identity, 1.0);

        let result = acc.finalize();

        // Output should be 200x200
        assert_eq!(result.image.width(), 200);
        assert_eq!(result.image.height(), 200);

        // Average value should be preserved (approximately)
        let pixels = result.image.pixels();
        let avg: f32 = pixels.iter().sum::<f32>() / pixels.len() as f32;
        // With pixfrac=0.8 and scale=2, not all pixels get full coverage
        // but well-covered pixels should have ~0.5 value
        assert!(
            avg > 0.3 && avg < 0.7,
            "Average value {} not in expected range",
            avg
        );
    }

    #[test]
    fn test_drizzle_point_kernel() {
        let image = AstroImage::from_pixels(10, 10, 1, vec![1.0; 10 * 10]);

        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
        let mut acc = DrizzleAccumulator::new(10, 10, 1, config);

        let identity = TransformMatrix::identity();
        acc.add_image(&image, &identity, 1.0);

        let result = acc.finalize();
        assert_eq!(result.image.width(), 20);
        assert_eq!(result.image.height(), 20);
    }

    #[test]
    fn test_drizzle_stack_empty_paths() {
        let paths: Vec<std::path::PathBuf> = vec![];
        let transforms: Vec<TransformMatrix> = vec![];
        let config = DrizzleConfig::default();

        let result = drizzle_stack(
            &paths,
            &transforms,
            None,
            &config,
            ProgressCallback::default(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_drizzle_rgb_image() {
        // Create a simple RGB test image
        let mut pixels = vec![0.0f32; 50 * 50 * 3];
        for y in 0..50 {
            for x in 0..50 {
                let idx = (y * 50 + x) * 3;
                pixels[idx] = 0.5; // R
                pixels[idx + 1] = 0.3; // G
                pixels[idx + 2] = 0.7; // B
            }
        }
        let image = AstroImage::from_pixels(50, 50, 3, pixels);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(50, 50, 3, config);

        let identity = TransformMatrix::identity();
        acc.add_image(&image, &identity, 1.0);

        let result = acc.finalize();

        assert_eq!(result.image.width(), 100);
        assert_eq!(result.image.height(), 100);
        assert_eq!(result.image.channels(), 3);
    }

    #[test]
    fn test_drizzle_with_translation() {
        // Create test image with known pattern
        let mut pixels = vec![0.0f32; 20 * 20];
        pixels[10 * 20 + 10] = 1.0; // Single bright pixel at center
        let image = AstroImage::from_pixels(20, 20, 1, pixels);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(20, 20, 1, config);

        // Add with small translation
        let transform = TransformMatrix::translation(0.5, 0.5);
        acc.add_image(&image, &transform, 1.0);

        let result = acc.finalize();

        // The bright pixel should appear shifted in the output
        assert_eq!(result.image.width(), 40);
        assert_eq!(result.image.height(), 40);
    }
}
