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

use std::path::Path;

use arrayvec::ArrayVec;
use glam::DVec2;
use rayon::prelude::*;

use crate::ImageDimensions;
use crate::astro_image::AstroImage;
use crate::common::Buffer2;
use crate::registration::transform::Transform;
use crate::stacking::progress::report_progress;
use crate::stacking::{Error, ProgressCallback, StackingStage};

/// Maximum number of channels (RGB = 3).
const MAX_CHANNELS: usize = 3;

/// Drizzle kernel type for distributing flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrizzleKernel {
    /// Turbo kernel: axis-aligned rectangular drop centered on the transformed pixel center.
    /// Approximation of true square kernel — always aligned with output X/Y axes regardless
    /// of rotation. Fast and adequate when rotation between frames is small. Default.
    /// (Named "turbo" in STScI DrizzlePac; "square" there uses full polygon clipping.)
    #[default]
    Turbo,
    /// Point kernel - single pixel contribution.
    /// Fastest but requires very good dithering.
    Point,
    /// Gaussian droplet with configurable FWHM.
    /// Smoother output, slight flux redistribution.
    Gaussian,
    /// Lanczos kernel for high-quality interpolation.
    /// Best quality but slowest. Only valid at pixfrac=1.0, scale=1.0.
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
            kernel: DrizzleKernel::Turbo,
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
    /// Accumulated weighted flux values, one Buffer2 per channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated weights, one Buffer2 per channel.
    weights: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Configuration.
    config: DrizzleConfig,
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for the given input dimensions.
    pub fn new(input_dims: ImageDimensions, config: DrizzleConfig) -> Self {
        assert!(
            input_dims.channels <= MAX_CHANNELS,
            "channels ({}) exceeds MAX_CHANNELS ({})",
            input_dims.channels,
            MAX_CHANNELS
        );
        let output_width = (input_dims.width as f32 * config.scale).ceil() as usize;
        let output_height = (input_dims.height as f32 * config.scale).ceil() as usize;

        let mut data = ArrayVec::new();
        let mut weights = ArrayVec::new();
        for _ in 0..input_dims.channels {
            data.push(Buffer2::new_default(output_width, output_height));
            weights.push(Buffer2::new_default(output_width, output_height));
        }

        Self {
            data,
            weights,
            config,
        }
    }

    fn width(&self) -> usize {
        self.data[0].width()
    }

    fn height(&self) -> usize {
        self.data[0].height()
    }

    fn channels(&self) -> usize {
        self.data.len()
    }

    /// Output dimensions.
    pub fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new(self.width(), self.height(), self.channels())
    }

    /// Add an image to the drizzle accumulator with the given transform.
    ///
    /// The transform maps input pixel coordinates to reference (output) coordinates.
    /// It should include any registration alignment computed from star matching.
    ///
    /// `pixel_weights` is an optional per-pixel weight map (same dimensions as input image).
    /// Values of 0.0 fully exclude a pixel (e.g. hot/dead pixels, cosmic rays);
    /// 1.0 is normal weight; intermediate values allow soft weighting.
    pub fn add_image(
        &mut self,
        image: AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
    ) {
        let n_channels = self.channels();
        assert_eq!(
            image.channels(),
            n_channels,
            "Channel count mismatch: expected {}, got {}",
            n_channels,
            image.channels()
        );

        if let Some(pw) = pixel_weights {
            assert_eq!(
                (pw.width(), pw.height()),
                (image.width(), image.height()),
                "Pixel weight map dimensions ({}x{}) must match image ({}x{})",
                pw.width(),
                pw.height(),
                image.width(),
                image.height()
            );
        }

        let scale = self.config.scale;
        let pixfrac = self.config.pixfrac;
        // Drop size in output pixels: pixfrac is the fraction of input pixel size,
        // and each input pixel maps to `scale` output pixels, so drop = pixfrac * scale.
        // (STScI: pfo = pixel_fraction / pscale_ratio / 2, where pscale_ratio = 1/scale)
        let drop_size = pixfrac * scale;

        if self.config.kernel == DrizzleKernel::Lanczos
            && ((pixfrac - 1.0).abs() > f32::EPSILON || (scale - 1.0).abs() > f32::EPSILON)
        {
            // Per STScI DrizzlePac: Lanczos "should never be used for pixfrac != 1.0,
            // and is not recommended for scale != 1.0."
            tracing::warn!(
                pixfrac,
                scale,
                "Lanczos kernel should only be used with pixfrac=1.0 and scale=1.0"
            );
        }

        match self.config.kernel {
            DrizzleKernel::Turbo => {
                self.add_image_turbo(&image, transform, weight, pixel_weights, scale, drop_size);
            }
            DrizzleKernel::Point => {
                self.add_image_point(&image, transform, weight, pixel_weights, scale);
            }
            DrizzleKernel::Gaussian => {
                // Per STScI: Gaussian FWHM = drop_size in output pixels.
                // sigma = FWHM / (2 * sqrt(2 * ln(2))) = FWHM / 2.3548
                let sigma = drop_size / 2.3548;
                let radius = (3.0 * sigma).ceil() as isize;
                let inv_2sigma_sq = 1.0 / (2.0 * sigma * sigma);
                self.add_image_radial(
                    &image,
                    transform,
                    weight,
                    pixel_weights,
                    scale,
                    radius,
                    |dx, dy| {
                        let dist_sq = dx * dx + dy * dy;
                        (-dist_sq * inv_2sigma_sq).exp()
                    },
                );
            }
            DrizzleKernel::Lanczos => {
                // Lanczos-3: support radius 3, kernel defined on [-3, 3].
                let a = 3.0f32;
                self.add_image_radial(
                    &image,
                    transform,
                    weight,
                    pixel_weights,
                    scale,
                    a as isize,
                    |dx, dy| lanczos_kernel(dx, a) * lanczos_kernel(dy, a),
                );
            }
        }
    }

    /// Add image using turbo kernel (axis-aligned rectangular drop).
    fn add_image_turbo(
        &mut self,
        image: &AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
        drop_size: f32,
    ) {
        let half_drop = drop_size / 2.0;
        let inv_area = 1.0 / (drop_size * drop_size);
        let output_width = self.width();
        let output_height = self.height();
        let input_width = image.width();
        let input_height = image.height();

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                let t = transform.apply(DVec2::new(ix as f64 + 0.5, iy as f64 + 0.5));
                let ox_center = t.x as f32 * scale;
                let oy_center = t.y as f32 * scale;

                let ox_min = (ox_center - half_drop).floor().max(0.0) as usize;
                let oy_min = (oy_center - half_drop).floor().max(0.0) as usize;
                let ox_max = (ox_center + half_drop).ceil().min(output_width as f32) as usize;
                let oy_max = (oy_center + half_drop).ceil().min(output_height as f32) as usize;

                let effective_weight = weight * pw;
                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
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
                            let pixel_weight = effective_weight * overlap * inv_area;
                            self.accumulate(image, ix, iy, ox, oy, pixel_weight);
                        }
                    }
                }
            }
        }
    }

    /// Add image using point kernel (fastest, needs good dithering).
    fn add_image_point(
        &mut self,
        image: &AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
    ) {
        let output_width = self.width();
        let output_height = self.height();
        let input_width = image.width();
        let input_height = image.height();

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                let t = transform.apply(DVec2::new(ix as f64 + 0.5, iy as f64 + 0.5));
                let ox = (t.x as f32 * scale).floor() as isize;
                let oy = (t.y as f32 * scale).floor() as isize;

                if ox >= 0 && ox < output_width as isize && oy >= 0 && oy < output_height as isize {
                    self.accumulate(image, ix, iy, ox as usize, oy as usize, weight * pw);
                }
            }
        }
    }

    /// Add image using a radial kernel with two-pass normalization.
    ///
    /// Shared implementation for Gaussian and Lanczos kernels. Both iterate output
    /// pixels within `radius` of the transformed center, compute a per-pixel weight
    /// via `kernel_fn(dx, dy)`, normalize so weights sum to 1, then accumulate.
    #[allow(clippy::too_many_arguments)]
    fn add_image_radial(
        &mut self,
        image: &AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
        radius: isize,
        kernel_fn: impl Fn(f32, f32) -> f32,
    ) {
        let output_width = self.width() as isize;
        let output_height = self.height() as isize;
        let input_width = image.width();
        let input_height = image.height();

        for iy in 0..input_height {
            for ix in 0..input_width {
                let pw = pixel_weights.map_or(1.0, |w| w[(ix, iy)]);
                if pw == 0.0 {
                    continue;
                }

                let t = transform.apply(DVec2::new(ix as f64 + 0.5, iy as f64 + 0.5));
                let ox_center = t.x as f32 * scale;
                let oy_center = t.y as f32 * scale;

                let ox_int = ox_center.floor() as isize;
                let oy_int = oy_center.floor() as isize;

                // First pass: compute total weight for normalization
                // (kernel geometry only — per-pixel weight applied to the frame weight)
                let mut total_weight = 0.0f32;
                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width {
                            continue;
                        }
                        let dist_x = (ox as f32 + 0.5) - ox_center;
                        let dist_y = (oy as f32 + 0.5) - oy_center;
                        total_weight += kernel_fn(dist_x, dist_y);
                    }
                }

                if total_weight.abs() < 1e-10 {
                    continue;
                }

                // Second pass: distribute flux with normalized weights.
                // Per-pixel weight scales the effective frame weight.
                let inv_total = (weight * pw) / total_weight;
                for dy in -radius..=radius {
                    let oy = oy_int + dy;
                    if oy < 0 || oy >= output_height {
                        continue;
                    }
                    for dx in -radius..=radius {
                        let ox = ox_int + dx;
                        if ox < 0 || ox >= output_width {
                            continue;
                        }
                        let dist_x = (ox as f32 + 0.5) - ox_center;
                        let dist_y = (oy as f32 + 0.5) - oy_center;
                        let pixel_weight = kernel_fn(dist_x, dist_y) * inv_total;
                        self.accumulate(image, ix, iy, ox as usize, oy as usize, pixel_weight);
                    }
                }
            }
        }
    }

    /// Accumulate weighted flux from input pixel (ix, iy) into output pixel (ox, oy).
    #[inline]
    fn accumulate(
        &mut self,
        image: &AstroImage,
        ix: usize,
        iy: usize,
        ox: usize,
        oy: usize,
        pixel_weight: f32,
    ) {
        for (c, (d, w)) in self
            .data
            .iter_mut()
            .zip(self.weights.iter_mut())
            .enumerate()
        {
            let flux = image.channel(c)[(ix, iy)];
            *d.get_mut(ox, oy) += flux * pixel_weight;
            *w.get_mut(ox, oy) += pixel_weight;
        }
    }

    /// Finalize the drizzle result, normalizing by coverage weights.
    pub fn finalize(self) -> DrizzleResult {
        let width = self.width();
        let height = self.height();
        let n_channels = self.channels();
        let needs_clamping = self.config.kernel == DrizzleKernel::Lanczos;
        let min_coverage = self.config.min_coverage;
        let fill_value = self.config.fill_value;

        // Coverage from channel 0 weights (all channels share identical geometric overlap).
        let weights0 = self.weights[0].pixels();
        let mut coverage = Buffer2::new_default(width, height);
        coverage.pixels_mut().copy_from_slice(weights0);

        // Find max weight for normalizing min_coverage threshold
        let max_weight = coverage
            .pixels()
            .par_iter()
            .copied()
            .reduce(|| 0.0f32, f32::max);

        // min_coverage is 0.0-1.0, compare against normalized weight
        let weight_threshold = if max_weight > 0.0 {
            min_coverage * max_weight
        } else {
            0.0
        };

        // Build per-channel output (row-parallel normalization)
        let output_channels: Vec<Vec<f32>> = (0..n_channels)
            .map(|c| {
                let data_pixels = self.data[c].pixels();
                let weight_pixels = self.weights[c].pixels();
                let mut out = vec![fill_value; width * height];

                out.par_chunks_mut(width)
                    .enumerate()
                    .for_each(|(y, out_row)| {
                        let row_start = y * width;
                        for (x, out_val) in out_row.iter_mut().enumerate() {
                            let idx = row_start + x;
                            let w = weight_pixels[idx];
                            if w >= weight_threshold && w > 0.0 {
                                let mut val = data_pixels[idx] / w;
                                if needs_clamping {
                                    val = val.max(0.0);
                                }
                                *out_val = val;
                            }
                        }
                    });

                out
            })
            .collect();

        // Normalize coverage to [0, 1]
        if max_weight > 0.0 {
            let inv_max = 1.0 / max_weight;
            coverage
                .pixels_mut()
                .par_iter_mut()
                .for_each(|c| *c *= inv_max);
        }

        let image = AstroImage::from_planar_channels(
            ImageDimensions::new(width, height, n_channels),
            output_channels,
        );

        DrizzleResult { image, coverage }
    }
}

/// Result of drizzle stacking.
#[derive(Debug)]
pub struct DrizzleResult {
    /// The drizzled output image.
    pub image: AstroImage,
    /// Normalized coverage map (0.0 = no data, 1.0 = maximum coverage).
    pub coverage: Buffer2<f32>,
}

impl DrizzleResult {
    /// Get coverage at a specific pixel.
    pub fn coverage_at(&self, x: usize, y: usize) -> f32 {
        self.coverage[(x, y)]
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
/// * `pixel_weight_maps` - Optional per-pixel weight maps (one per frame, same dims as input).
///   Values of 0.0 exclude pixels (bad pixels, cosmic rays); 1.0 is normal.
/// * `config` - Drizzle configuration
/// * `progress` - Progress callback
///
/// # Returns
///
/// The drizzled result with image and coverage map.
pub fn drizzle_stack<P: AsRef<Path> + Sync>(
    paths: &[P],
    transforms: &[Transform],
    weights: Option<&[f32]>,
    pixel_weight_maps: Option<&[Buffer2<f32>]>,
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

    if let Some(pw) = pixel_weight_maps {
        assert_eq!(
            paths.len(),
            pw.len(),
            "Number of paths ({}) must match number of pixel weight maps ({})",
            paths.len(),
            pw.len()
        );
    }

    // Load first image to get dimensions
    let first_image = AstroImage::from_file(paths[0].as_ref()).map_err(|e| Error::ImageLoad {
        path: paths[0].as_ref().to_path_buf(),
        source: std::io::Error::other(e.to_string()),
    })?;

    let input_dims = first_image.dimensions;

    tracing::info!(
        input_width = input_dims.width,
        input_height = input_dims.height,
        channels = input_dims.channels,
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        frame_count = paths.len(),
        "Starting drizzle stacking"
    );

    let mut accumulator = DrizzleAccumulator::new(input_dims, config.clone());
    let out_dims = accumulator.dimensions();
    tracing::info!(
        output_width = out_dims.width,
        output_height = out_dims.height,
        "Output dimensions"
    );

    // Add first image
    let first_weight = weights.map_or(1.0, |w| w[0]);
    let first_pw = pixel_weight_maps.map(|maps| &maps[0]);
    accumulator.add_image(first_image, &transforms[0], first_weight, first_pw);
    report_progress(&progress, 1, paths.len(), StackingStage::Processing);

    // Process remaining images
    for (i, path) in paths.iter().enumerate().skip(1) {
        let image = AstroImage::from_file(path.as_ref()).map_err(|e| Error::ImageLoad {
            path: path.as_ref().to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })?;

        // Validate dimensions match
        if image.width() != input_dims.width || image.height() != input_dims.height {
            return Err(Error::ImageLoad {
                path: path.as_ref().to_path_buf(),
                source: std::io::Error::other(format!(
                    "Dimension mismatch: expected {}x{}, got {}x{}",
                    input_dims.width,
                    input_dims.height,
                    image.width(),
                    image.height()
                )),
            });
        }

        let weight = weights.map_or(1.0, |w| w[i]);
        let pw = pixel_weight_maps.map(|maps| &maps[i]);
        accumulator.add_image(image, &transforms[i], weight, pw);
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
        assert_eq!(config.kernel, DrizzleKernel::Turbo);
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
        let acc = DrizzleAccumulator::new(ImageDimensions::new(100, 80, 3), config);
        let dims = acc.dimensions();
        assert_eq!(dims.width, 200);
        assert_eq!(dims.height, 160);
        assert_eq!(dims.channels, 3);
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
    fn test_drizzle_single_image() {
        // Create a simple test image
        let image =
            AstroImage::from_pixels(ImageDimensions::new(100, 100, 1), vec![0.5; 100 * 100]);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(100, 100, 1), config);

        let identity = Transform::identity();
        acc.add_image(image, &identity, 1.0, None);

        let result = acc.finalize();

        // Output should be 200x200
        assert_eq!(result.image.width(), 200);
        assert_eq!(result.image.height(), 200);

        // With scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6 output pixels.
        // Input pixel (ix,iy) center at (ix+0.5, iy+0.5), scaled to (2*ix+1, 2*iy+1).
        // Drop covers (center ± 0.8), spanning a 2×2 output block.
        // Each overlap = 0.8 * 0.8 = 0.64, inv_area = 1/2.56, weight = 0.64/2.56 = 0.25.
        // Interior output pixels receive exactly one contribution.
        // Finalized output = (0.5 * 0.25) / 0.25 = 0.5
        let pixels = result.image.channel(0);
        let avg: f32 = pixels.iter().sum::<f32>() / pixels.len() as f32;
        assert!(
            (avg - 0.5).abs() < 1e-5,
            "Average should be 0.5, got {}",
            avg
        );
        // Verify specific pixels
        assert!(
            (pixels[0] - 0.5).abs() < 1e-5,
            "Pixel (0,0) should be 0.5, got {}",
            pixels[0]
        );
    }

    #[test]
    fn test_drizzle_point_kernel() {
        let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![1.0; 10 * 10]);

        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config);

        let identity = Transform::identity();
        acc.add_image(image, &identity, 1.0, None);

        let result = acc.finalize();
        assert_eq!(result.image.width(), 20);
        assert_eq!(result.image.height(), 20);

        // Point kernel: input (ix,iy) center at (ix+0.5, iy+0.5)
        // scaled → output floor((ix+0.5)*2) = 2*ix+1, floor((iy+0.5)*2) = 2*iy+1
        // Covered pixels (odd x, odd y): value = 1.0/1.0 = 1.0
        // Uncovered pixels: fill_value = 0.0
        let pixels = result.image.channel(0);
        let w = 20;
        // (1,1) ← input (0,0): value = 1.0
        assert!((pixels[w + 1] - 1.0).abs() < f32::EPSILON);
        // (3,1) ← input (1,0): value = 1.0
        assert!((pixels[w + 3] - 1.0).abs() < f32::EPSILON);
        // (0,0): no coverage → fill_value = 0.0
        assert!((pixels[0]).abs() < f32::EPSILON);
        // (2,2): even coords, no coverage → 0.0
        assert!((pixels[2 * w + 2]).abs() < f32::EPSILON);
        // Exactly 100 covered pixels (10×10 input maps to 10×10 odd-coordinate outputs)
        let covered = pixels.iter().filter(|&&v| v > 0.5).count();
        assert_eq!(covered, 100);
    }

    #[test]
    fn test_drizzle_stack_empty_paths() {
        let paths: Vec<std::path::PathBuf> = vec![];
        let transforms: Vec<Transform> = vec![];
        let config = DrizzleConfig::default();

        let result = drizzle_stack(
            &paths,
            &transforms,
            None,
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
        let image = AstroImage::from_pixels(ImageDimensions::new(50, 50, 3), pixels);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(50, 50, 3), config);

        let identity = Transform::identity();
        acc.add_image(image, &identity, 1.0, None);

        let result = acc.finalize();

        assert_eq!(result.image.width(), 100);
        assert_eq!(result.image.height(), 100);
        assert_eq!(result.image.channels(), 3);
    }

    #[test]
    fn test_drizzle_with_translation() {
        // Single bright pixel at (10,10), all others zero
        let mut pixels = vec![0.0f32; 20 * 20];
        pixels[10 * 20 + 10] = 1.0;
        let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), pixels);

        // scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6, half_drop = 0.8
        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);

        // Translation (0.5, 0.5): pixel (10,10) center at (10.5+0.5, 10.5+0.5) = (11, 11)
        // Scaled to output: (22, 22). Drop from (21.2, 21.2) to (22.8, 22.8).
        // Overlaps output pixels (21,21), (22,21), (21,22), (22,22).
        // Each overlap = 0.8 * 0.8 = 0.64, inv_area = 1/2.56, weight = 0.25.
        let transform = Transform::translation(DVec2::new(0.5, 0.5));
        acc.add_image(image, &transform, 1.0, None);

        let result = acc.finalize();
        assert_eq!(result.image.width(), 40);
        assert_eq!(result.image.height(), 40);

        let out = result.image.channel(0);
        // The 4 output pixels receiving flux = 1.0*0.25/0.25 = 1.0
        assert!(
            (out[21 * 40 + 21] - 1.0).abs() < 1e-5,
            "Expected 1.0 at (21,21), got {}",
            out[21 * 40 + 21]
        );
        assert!(
            (out[21 * 40 + 22] - 1.0).abs() < 1e-5,
            "Expected 1.0 at (22,21), got {}",
            out[21 * 40 + 22]
        );
        assert!(
            (out[22 * 40 + 21] - 1.0).abs() < 1e-5,
            "Expected 1.0 at (21,22), got {}",
            out[22 * 40 + 21]
        );
        assert!(
            (out[22 * 40 + 22] - 1.0).abs() < 1e-5,
            "Expected 1.0 at (22,22), got {}",
            out[22 * 40 + 22]
        );
        // Pixel far from the bright spot should be 0.0
        assert!((out[0]).abs() < 1e-5);
    }

    #[test]
    fn test_coverage_at() {
        // Point kernel with identity: covered at odd coords, uncovered at even
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 16]);
        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        // Output 8×8. Covered pixels at (2*ix+1, 2*iy+1) for ix,iy=0..3
        // Normalized coverage: max_coverage = 1.0
        assert!((result.coverage_at(1, 1) - 1.0).abs() < f32::EPSILON); // covered
        assert!((result.coverage_at(0, 0)).abs() < f32::EPSILON); // uncovered
        assert!((result.coverage_at(3, 3) - 1.0).abs() < f32::EPSILON); // covered
        assert!((result.coverage_at(2, 2)).abs() < f32::EPSILON); // uncovered
    }

    /// Test turbo kernel drop size and overlap with hand-computed values.
    ///
    /// Setup: 4×4 input, scale=2, pixfrac=0.8 → drop_size = 1.6, half_drop = 0.8
    /// Output: 8×8. Input pixel (1,1) center at (1.5, 1.5), scaled to (3.0, 3.0).
    /// Drop covers (2.2, 2.2) to (3.8, 3.8).
    ///
    /// Overlapping output pixels and their overlap areas:
    ///   (2,2): x_overlap = min(3.8,3)-max(2.2,2) = 0.8, y = 0.8, area = 0.64
    ///   (3,2): x_overlap = min(3.8,4)-max(2.2,3) = 0.8, y = 0.8, area = 0.64
    ///   (2,3): same as (3,2) by symmetry, area = 0.64
    ///   (3,3): x = min(3.8,4)-max(2.2,3) = 0.8, y = 0.8, area = 0.64
    /// Total area = 4 * 0.64 = 2.56 = 1.6^2 ✓
    /// inv_area = 1/2.56 ≈ 0.390625
    /// pixel_weight = overlap * inv_area = 0.64 * 0.390625 = 0.25
    #[test]
    fn test_turbo_kernel_overlap_exact() {
        // Single bright pixel at (1,1) with value 2.0
        let mut pixels = vec![0.0f32; 4 * 4];
        pixels[5] = 2.0; // pixel (1,1) = index 1*4+1 = 5
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

        let config = DrizzleConfig::x2(); // scale=2, pixfrac=0.8
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);

        let result = acc.finalize();
        let out = result.image.channel(0);
        let w = 8usize;

        // Drop from (2.2,2.2) to (3.8,3.8) → overlaps (2,2), (3,2), (2,3), (3,3).
        // Each receives: flux=2.0, weight=0.25 → finalized = 2.0*0.25/0.25 = 2.0
        assert!(
            (out[2 * w + 2] - 2.0).abs() < 1e-5,
            "Expected 2.0 at (2,2), got {}",
            out[2 * w + 2]
        );
        assert!(
            (out[2 * w + 3] - 2.0).abs() < 1e-5,
            "Expected 2.0 at (3,2), got {}",
            out[2 * w + 3]
        );
        assert!(
            (out[3 * w + 2] - 2.0).abs() < 1e-5,
            "Expected 2.0 at (2,3), got {}",
            out[3 * w + 2]
        );
        assert!(
            (out[3 * w + 3] - 2.0).abs() < 1e-5,
            "Expected 2.0 at (3,3), got {}",
            out[3 * w + 3]
        );

        // Adjacent pixels outside the drop should be zero (or fill_value)
        assert!(out[w + 2].abs() < 1e-5, "No flux at (2,1)");
        assert!(out[4 * w + 3].abs() < 1e-5, "No flux at (3,4)");
    }

    /// Test turbo kernel with non-integer translation producing asymmetric overlap.
    ///
    /// Uniform image value=1.0, translation=(0.25, 0.0).
    /// scale=2, pixfrac=1.0 → drop_size = 2.0, half_drop = 1.0
    /// Pixel (0,0) center at (0.5+0.25, 0.5) = (0.75, 0.5), scaled to (1.5, 1.0).
    /// Drop from (0.5, 0.0) to (2.5, 2.0).
    ///
    /// Output pixel (0,0) receives contributions from input (0,0) only:
    ///   area = 0.5 * 1.0 = 0.5, weight = 0.5 * 0.25 = 0.125
    /// Output pixel (1,0) receives from (0,0) and (1,0):
    ///   from (0,0): area=1.0, weight=0.25; from (1,0): area=1.0, weight=0.25
    ///   total data = 1.0*0.25 + 1.0*0.25 = 0.5, total weight = 0.5 → output = 1.0
    ///
    /// For uniform image, all covered pixels should be 1.0 (weighted mean of same value).
    /// Coverage varies: (0,0) has weight 0.125, (1,0) has weight 0.50.
    #[test]
    fn test_turbo_kernel_fractional_shift() {
        // Uniform image so weighted mean is always 1.0 regardless of overlap pattern
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 4 * 4]);

        let config = DrizzleConfig::x2().with_pixfrac(1.0); // drop_size = 2.0
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        let transform = Transform::translation(DVec2::new(0.25, 0.0));
        acc.add_image(image, &transform, 1.0, None);

        let result = acc.finalize();
        let out = result.image.channel(0);
        let w = 8usize;

        // All covered interior pixels should be 1.0
        assert!(
            (out[2 * w + 3] - 1.0).abs() < 1e-5,
            "Interior pixel should be 1.0, got {}",
            out[2 * w + 3]
        );

        // Verify asymmetric coverage from the fractional shift.
        // Input pixel (0,0) → output center (1.5, 1.0). Drop (0.5, 0.0)→(2.5, 2.0).
        // Output (0,0) gets weight 0.5*1.0*0.25 = 0.125 from input (0,0) only.
        // Output (1,0) gets weight 1.0*1.0*0.25 = 0.25 from input (0,0) AND (1,0).
        // So total weight at (1,0) = 0.25+0.25 = 0.50.
        // Coverage normalization: max_weight across all pixels.
        // Interior pixel (3,2) gets contributions from two input pixels, total weight 0.50.
        // Coverage at edge (0,0) = 0.125 / max_weight.
        // The exact max depends on full overlap pattern — just verify (0,0) < (1,0).
        assert!(
            result.coverage_at(0, 0) < result.coverage_at(1, 0),
            "Edge pixel should have less coverage than interior"
        );
    }

    /// Test that min_coverage works with normalized weights.
    ///
    /// With pixfrac=1.0, scale=2: single frame, max weight = 0.25.
    /// min_coverage=0.6: threshold = 0.6 * 0.25 = 0.15.
    /// A pixel with overlap 0.5 → weight = 0.5*0.25 = 0.125 < 0.15 → rejected.
    /// A pixel with overlap 1.0 → weight = 1.0*0.25 = 0.25 >= 0.15 → kept.
    #[test]
    fn test_min_coverage_normalized() {
        // Single pixel at (0,0) with fractional shift to create unequal overlaps
        let mut pixels = vec![0.0f32; 4 * 4];
        pixels[0] = 1.0;
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

        let config = DrizzleConfig::x2().with_pixfrac(1.0).with_min_coverage(0.6);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);

        // Translation (0.25, 0): creates overlaps of 0.5 and 1.0 (see previous test)
        let transform = Transform::translation(DVec2::new(0.25, 0.0));
        acc.add_image(image, &transform, 1.0, None);
        let result = acc.finalize();
        let out = result.image.channel(0);

        // Pixel (1,0) has weight 0.25 (max), pixel (0,0) has weight 0.125.
        // Threshold = 0.6 * 0.25 = 0.15. So (0,0) with 0.125 < 0.15 → fill_value.
        // Pixel (1,0) with 0.25 >= 0.15 → kept.
        assert!((out[1] - 1.0).abs() < 1e-5, "Pixel (1,0) kept: {}", out[1]);
        assert!(
            out[0].abs() < 1e-5,
            "Pixel (0,0) rejected (below min_coverage): {}",
            out[0]
        );
    }

    /// Test Gaussian kernel produces flux-preserving smooth output.
    ///
    /// A uniform 10×10 image with value 3.0 through Gaussian kernel should produce
    /// approximately 3.0 everywhere in the interior (edges may differ due to truncation).
    #[test]
    fn test_gaussian_kernel_uniform_preserves_value() {
        let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![3.0; 10 * 10]);

        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        // Interior pixels should be ≈ 3.0 (Gaussian is normalized per-pixel)
        let out = result.image.channel(0);
        let w = 20usize;
        // Check a pixel well inside the interior
        let center_val = out[10 * w + 10];
        assert!(
            (center_val - 3.0).abs() < 0.05,
            "Interior Gaussian value should be ~3.0, got {}",
            center_val
        );
    }

    /// Test Lanczos kernel with scale=1, pixfrac=1.
    ///
    /// For a uniform image at scale=1 pixfrac=1, Lanczos should also produce the same
    /// uniform value, since the normalized Lanczos weights sum to 1.
    #[test]
    fn test_lanczos_kernel_uniform_preserves_value() {
        let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), vec![5.0; 20 * 20]);

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Lanczos,
            fill_value: 0.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        // Interior pixel well away from borders
        let center_val = out[10 * 20 + 10];
        assert!(
            (center_val - 5.0).abs() < 0.01,
            "Lanczos uniform should be ~5.0, got {}",
            center_val
        );
    }

    /// Test that Lanczos clamping prevents negative output.
    ///
    /// A single bright pixel surrounded by zeros will produce negative lobes
    /// in the Lanczos output. After clamping, all output should be >= 0.
    #[test]
    fn test_lanczos_clamping_no_negative_output() {
        let mut pixels = vec![0.0f32; 20 * 20];
        pixels[10 * 20 + 10] = 100.0; // bright point source
        let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), pixels);

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Lanczos,
            fill_value: 0.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        let min_val = out.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            min_val >= 0.0,
            "Lanczos output should be clamped to >= 0.0, got min={}",
            min_val
        );
    }

    /// Test two-frame accumulation with different weights.
    ///
    /// Frame 1: uniform value 2.0, weight 1.0
    /// Frame 2: uniform value 6.0, weight 3.0
    /// Expected weighted mean: (2.0*w1 + 6.0*w3) / (w1+w3) at each pixel.
    /// Since pixel_weight = frame_weight * overlap * inv_area, and both frames
    /// have the same overlap geometry, the weighted mean simplifies to:
    /// (2.0 * 1.0 + 6.0 * 3.0) / (1.0 + 3.0) = (2 + 18) / 4 = 5.0
    #[test]
    fn test_two_frame_weighted_mean() {
        let image1 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![2.0; 10 * 10]);
        let image2 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![6.0; 10 * 10]);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config);
        acc.add_image(image1, &Transform::identity(), 1.0, None);
        acc.add_image(image2, &Transform::identity(), 3.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        // Interior pixel
        let center_val = out[10 * 20 + 10];
        assert!(
            (center_val - 5.0).abs() < 1e-5,
            "Weighted mean should be 5.0, got {}",
            center_val
        );
    }

    /// Test pixfrac changes the drop size and thus the weight distribution.
    ///
    /// With scale=2 and non-integer-centered drop (via translation):
    /// pixfrac=1.0 → drop_size=2.0: large drop hits many output pixels
    /// pixfrac=0.3 → drop_size=0.6: small drop hits fewer output pixels
    #[test]
    fn test_pixfrac_changes_weight_distribution() {
        // Use translation (0.1, 0.1) to avoid integer-centered drops
        let transform = Transform::translation(DVec2::new(0.1, 0.1));

        // pixfrac=1.0: drop_size=2.0, half=1.0
        // Input (2,2) center (2.6,2.6) → output (5.2,5.2). Drop (4.2,4.2)→(6.2,6.2).
        // Covers output pixels (4,4),(5,4),(6,4),(4,5),(5,5),(6,5),(4,6),(5,6),(6,6) = 9 pixels
        let mut pixels1 = vec![0.0f32; 6 * 6];
        pixels1[2 * 6 + 2] = 1.0;
        let image1 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), pixels1);

        let config1 = DrizzleConfig::x2().with_pixfrac(1.0);
        let mut acc1 = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config1);
        acc1.add_image(image1, &transform, 1.0, None);
        let r1 = acc1.finalize();
        let out1 = r1.image.channel(0);
        let covered_1 = out1.iter().filter(|&&v| v > 0.01).count();

        // pixfrac=0.3: drop_size=0.6, half=0.3
        // Drop (4.9,4.9)→(5.5,5.5): overlaps (4,4),(5,4),(4,5),(5,5) = 4 pixels
        let mut pixels2 = vec![0.0f32; 6 * 6];
        pixels2[2 * 6 + 2] = 1.0;
        let image2 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), pixels2);

        let config2 = DrizzleConfig::x2().with_pixfrac(0.3);
        let mut acc2 = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config2);
        acc2.add_image(image2, &transform, 1.0, None);
        let r2 = acc2.finalize();
        let out2 = r2.image.channel(0);
        let covered_2 = out2.iter().filter(|&&v| v > 0.01).count();

        // pixfrac=1.0 should cover more output pixels than pixfrac=0.3
        assert!(
            covered_1 > covered_2,
            "pixfrac=1.0 should cover more pixels ({}) than pixfrac=0.3 ({})",
            covered_1,
            covered_2
        );
    }

    /// Test RGB channels are handled independently.
    #[test]
    fn test_rgb_channels_independent() {
        let mut pixels = vec![0.0f32; 4 * 4 * 3];
        // Set pixel (1,1) to (1.0, 2.0, 3.0). Index = (1*4+1)*3 = 15.
        let idx = 15;
        pixels[idx] = 1.0;
        pixels[idx + 1] = 2.0;
        pixels[idx + 2] = 3.0;
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 3), pixels);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 3), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        // Pixel (2,2) in output should have (1.0, 2.0, 3.0) (normalized by equal weight)
        let r = result.image.channel(0);
        let g = result.image.channel(1);
        let b = result.image.channel(2);
        let w = 8usize;
        assert!(
            (r[2 * w + 2] - 1.0).abs() < 1e-5,
            "R should be 1.0, got {}",
            r[2 * w + 2]
        );
        assert!(
            (g[2 * w + 2] - 2.0).abs() < 1e-5,
            "G should be 2.0, got {}",
            g[2 * w + 2]
        );
        assert!(
            (b[2 * w + 2] - 3.0).abs() < 1e-5,
            "B should be 3.0, got {}",
            b[2 * w + 2]
        );
    }

    /// Test scale=1 with pixfrac=1 (shift-and-add equivalent).
    ///
    /// At scale=1, pixfrac=1: drop_size=1.0, covering exactly one output pixel per input pixel.
    /// With identity transform, output should exactly equal input.
    #[test]
    fn test_scale1_pixfrac1_identity() {
        let pixels: Vec<f32> = (0..25).map(|i| i as f32).collect();
        let image = AstroImage::from_pixels(ImageDimensions::new(5, 5, 1), pixels.clone());

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Turbo,
            fill_value: 0.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(5, 5, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        // Each input pixel center (ix+0.5, iy+0.5) → same in output.
        // drop_size=1.0, half=0.5: drop from (ix, iy) to (ix+1, iy+1).
        // Overlaps exactly output pixel (ix, iy) with area 1.0.
        // inv_area = 1.0. weight = 1.0. output = value * 1.0 / 1.0 = value.
        for (i, (&actual, &expected)) in out.iter().zip(pixels.iter()).enumerate() {
            assert!(
                (actual - expected).abs() < 1e-5,
                "Pixel {} should be {}, got {}",
                i,
                expected,
                actual
            );
        }
    }

    /// Test that custom fill_value appears in uncovered pixels.
    ///
    /// Point kernel at scale=2 leaves gaps (even-coordinate pixels uncovered).
    /// With fill_value = -999.0, those gaps should contain -999.0 instead of 0.0.
    #[test]
    fn test_fill_value_in_uncovered_pixels() {
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 16]);

        let config = DrizzleConfig {
            scale: 2.0,
            pixfrac: 0.8,
            kernel: DrizzleKernel::Point,
            fill_value: -999.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        let w = 8usize;

        // Point kernel: input (ix,iy) → output (2*ix+1, 2*iy+1).
        // Covered pixel (1,1): value = 1.0
        assert!(
            (out[w + 1] - 1.0).abs() < f32::EPSILON,
            "Covered pixel should be 1.0, got {}",
            out[w + 1]
        );
        // Uncovered pixel (0,0): fill_value = -999.0
        assert!(
            (out[0] - (-999.0)).abs() < f32::EPSILON,
            "Uncovered pixel should be -999.0, got {}",
            out[0]
        );
        // Uncovered pixel (2,2): fill_value = -999.0
        assert!(
            (out[2 * w + 2] - (-999.0)).abs() < f32::EPSILON,
            "Uncovered pixel (2,2) should be -999.0, got {}",
            out[2 * w + 2]
        );
    }

    /// Test that a zero-weight frame does not affect the output.
    ///
    /// Frame 1: uniform 3.0, weight 1.0
    /// Frame 2: uniform 100.0, weight 0.0
    /// Result should be 3.0 everywhere (zero-weight frame contributes nothing).
    #[test]
    fn test_zero_weight_frame_ignored() {
        let image1 = AstroImage::from_pixels(ImageDimensions::new(8, 8, 1), vec![3.0; 64]);
        let image2 = AstroImage::from_pixels(ImageDimensions::new(8, 8, 1), vec![100.0; 64]);

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(8, 8, 1), config);
        acc.add_image(image1, &Transform::identity(), 1.0, None);
        acc.add_image(image2, &Transform::identity(), 0.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        // Interior pixel: should be 3.0, not influenced by the 100.0 frame
        let center = out[8 * 16 + 8];
        assert!(
            (center - 3.0).abs() < 1e-5,
            "Zero-weight frame should not affect output, got {}",
            center
        );
    }

    /// Test Gaussian kernel with translation on a uniform image.
    ///
    /// Uniform value 4.0, translation (1.0, 0.5), scale=2, pixfrac=0.8.
    /// For a uniform image, the Gaussian-weighted mean at every interior pixel
    /// is 4.0 (weighted average of identical values).
    /// This tests that Gaussian + translation still preserves uniform values.
    #[test]
    fn test_gaussian_kernel_with_translation() {
        let image = AstroImage::from_pixels(ImageDimensions::new(12, 12, 1), vec![4.0; 144]);

        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(12, 12, 1), config);
        let transform = Transform::translation(DVec2::new(1.0, 0.5));
        acc.add_image(image, &transform, 1.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        let w = 24usize;

        // Interior pixel well inside the translated region: should be ~4.0
        let center = out[12 * w + 14];
        assert!(
            (center - 4.0).abs() < 0.05,
            "Gaussian interior with translation should be ~4.0, got {}",
            center
        );

        // Another interior pixel
        let other = out[10 * w + 10];
        assert!(
            (other - 4.0).abs() < 0.05,
            "Gaussian interior pixel should be ~4.0, got {}",
            other
        );

        // Pixel far outside the translated input region: should be fill_value (0.0)
        // Translation (1.0, 0.5) shifts input right+down. Output pixel (0,0) is far from any input.
        assert!(
            out[0].abs() < 1e-5,
            "Far pixel should be 0.0, got {}",
            out[0]
        );
    }

    /// Test Lanczos kernel with translation preserves value for a uniform image.
    ///
    /// Uniform image value 7.0, translation (0.3, -0.2), scale=1, pixfrac=1.
    /// Interior pixels should still be ~7.0 since the weighted mean of uniform values is invariant.
    #[test]
    fn test_lanczos_kernel_with_translation() {
        let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), vec![7.0; 400]);

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Lanczos,
            fill_value: 0.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
        let transform = Transform::translation(DVec2::new(0.3, -0.2));
        acc.add_image(image, &transform, 1.0, None);
        let result = acc.finalize();

        let out = result.image.channel(0);
        // Interior pixel well away from borders (Lanczos radius=3, so stay 4+ pixels inside)
        let center = out[10 * 20 + 10];
        assert!(
            (center - 7.0).abs() < 0.05,
            "Lanczos with translation should preserve uniform value ~7.0, got {}",
            center
        );

        // Another interior pixel
        let other = out[8 * 20 + 12];
        assert!(
            (other - 7.0).abs() < 0.05,
            "Lanczos interior pixel should be ~7.0, got {}",
            other
        );
    }

    /// Test that a pixel with weight 0.0 is fully excluded from the output.
    ///
    /// Setup: 4×4 image, pixel (1,1) = 100.0, all others = 1.0.
    /// Weight map: pixel (1,1) = 0.0, all others = 1.0.
    /// scale=1, pixfrac=1 (identity mapping, one-to-one).
    ///
    /// Without weight map: output (1,1) = 100.0.
    /// With weight map: pixel (1,1) contributes nothing. Output (1,1) receives
    /// only flux from neighboring drops that overlap it. With scale=1,pixfrac=1
    /// each input pixel maps to exactly one output pixel, so (1,1) gets no
    /// contribution at all → fill_value = 0.0.
    #[test]
    fn test_pixel_weight_zero_excludes_pixel() {
        let mut pixels = vec![1.0f32; 4 * 4];
        pixels[5] = 100.0; // (1,1) = row 1 * 4 + col 1 = 5 (bad pixel)
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

        let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
        *pw.get_mut(1, 1) = 0.0; // Exclude (1,1)

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Turbo,
            fill_value: 0.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
        let result = acc.finalize();
        let out = result.image.channel(0);

        // (1,1) = index 5, should be fill_value (0.0) — the bad pixel was excluded
        assert!(
            out[5].abs() < 1e-5,
            "Bad pixel (1,1) should be excluded, got {}",
            out[5]
        );
        // (0,0) should be 1.0 — normal pixel unaffected
        assert!(
            (out[0] - 1.0).abs() < 1e-5,
            "Normal pixel (0,0) should be 1.0, got {}",
            out[0]
        );
        // (2,2) should be 1.0
        assert!(
            (out[2 * 4 + 2] - 1.0).abs() < 1e-5,
            "Normal pixel (2,2) should be 1.0, got {}",
            out[2 * 4 + 2]
        );
    }

    /// Test that per-pixel weights correctly scale contributions in weighted mean.
    ///
    /// Two frames, same geometry (identity, scale=1, pixfrac=1):
    ///   Frame 1: all pixels = 2.0, pixel weight at (1,1) = 0.5
    ///   Frame 2: all pixels = 6.0, pixel weight at (1,1) = 1.0
    ///
    /// At (1,1): weighted mean = (2.0 * 0.5 + 6.0 * 1.0) / (0.5 + 1.0) = 7.0 / 1.5 = 4.667
    /// At other pixels: weighted mean = (2.0 * 1.0 + 6.0 * 1.0) / (1.0 + 1.0) = 4.0
    #[test]
    fn test_pixel_weight_scales_contribution() {
        let image1 = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![2.0; 16]);
        let image2 = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![6.0; 16]);

        let mut pw1 = Buffer2::new_filled(4, 4, 1.0f32);
        *pw1.get_mut(1, 1) = 0.5;
        let pw2 = Buffer2::new_filled(4, 4, 1.0f32);

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Turbo,
            fill_value: 0.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image1, &Transform::identity(), 1.0, Some(&pw1));
        acc.add_image(image2, &Transform::identity(), 1.0, Some(&pw2));
        let result = acc.finalize();
        let out = result.image.channel(0);

        // (1,1) = index 5: (2.0*0.5 + 6.0*1.0) / (0.5+1.0) = 7.0/1.5 = 4.6667
        let expected_11 = 7.0 / 1.5;
        assert!(
            (out[5] - expected_11).abs() < 1e-5,
            "Pixel (1,1) should be {:.4}, got {}",
            expected_11,
            out[5]
        );
        // (0,0): (2.0+6.0)/2.0 = 4.0
        assert!(
            (out[0] - 4.0).abs() < 1e-5,
            "Pixel (0,0) should be 4.0, got {}",
            out[0]
        );
    }

    /// Test bad pixel mask: multiple zero-weight pixels in a uniform field.
    ///
    /// 8×8 uniform image = 5.0. Weight map has 3 bad pixels at (2,3), (5,1), (7,7).
    /// scale=1, pixfrac=1. Bad pixels produce fill_value; all others = 5.0.
    #[test]
    fn test_pixel_weight_bad_pixel_mask() {
        let image = AstroImage::from_pixels(ImageDimensions::new(8, 8, 1), vec![5.0; 64]);

        let mut pw = Buffer2::new_filled(8, 8, 1.0f32);
        *pw.get_mut(2, 3) = 0.0;
        *pw.get_mut(5, 1) = 0.0;
        *pw.get_mut(7, 7) = 0.0;

        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel: DrizzleKernel::Turbo,
            fill_value: -1.0,
            min_coverage: 0.0,
        };
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(8, 8, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
        let result = acc.finalize();
        let out = result.image.channel(0);

        // Bad pixels → fill_value = -1.0
        assert!(
            (out[3 * 8 + 2] - (-1.0)).abs() < 1e-5,
            "Bad pixel (2,3) should be fill, got {}",
            out[3 * 8 + 2]
        );
        assert!(
            (out[13] - (-1.0)).abs() < 1e-5, // (5,1) = row 1 * 8 + col 5
            "Bad pixel (5,1) should be fill, got {}",
            out[13]
        );
        assert!(
            (out[7 * 8 + 7] - (-1.0)).abs() < 1e-5,
            "Bad pixel (7,7) should be fill, got {}",
            out[7 * 8 + 7]
        );

        // Good pixels → 5.0
        assert!(
            (out[0] - 5.0).abs() < 1e-5,
            "Good pixel (0,0) should be 5.0, got {}",
            out[0]
        );
        assert!(
            (out[4 * 8 + 4] - 5.0).abs() < 1e-5,
            "Good pixel (4,4) should be 5.0, got {}",
            out[4 * 8 + 4]
        );

        // Count: 3 bad pixels → fill, 61 good → 5.0
        let bad_count = out.iter().filter(|&&v| (v - (-1.0)).abs() < 1e-5).count();
        let good_count = out.iter().filter(|&&v| (v - 5.0).abs() < 1e-5).count();
        assert_eq!(bad_count, 3, "Expected 3 bad pixels");
        assert_eq!(good_count, 61, "Expected 61 good pixels");
    }

    /// Test per-pixel weights with point kernel.
    ///
    /// scale=2: input (ix,iy) → output (2*ix+1, 2*iy+1). Point kernel = 1 output pixel.
    /// Pixel (1,1) = 10.0, weight = 0.0. Should not appear at output (3,3).
    /// Pixel (0,0) = 3.0, weight = 1.0. Should appear at output (1,1) = 3.0.
    #[test]
    fn test_pixel_weight_with_point_kernel() {
        let mut pixels = vec![1.0f32; 4 * 4];
        pixels[5] = 10.0; // (1,1) bad pixel
        pixels[0] = 3.0;
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

        let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
        *pw.get_mut(1, 1) = 0.0;

        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
        let result = acc.finalize();
        let out = result.image.channel(0);
        let w = 8usize;

        // Output (3,3) ← input (1,1): excluded by weight=0 → fill_value 0.0
        assert!(
            out[3 * w + 3].abs() < 1e-5,
            "Excluded pixel output (3,3) should be 0.0, got {}",
            out[3 * w + 3]
        );
        // Output (1,1) ← input (0,0): weight=1.0, value=3.0
        assert!(
            (out[w + 1] - 3.0).abs() < 1e-5,
            "Good pixel output (1,1) should be 3.0, got {}",
            out[w + 1]
        );
    }

    /// Test per-pixel weights with Gaussian kernel on a uniform image.
    ///
    /// Uniform image = 4.0. One bad pixel at (5,5) with weight 0.0.
    /// Gaussian kernel spreads flux over multiple output pixels, so the bad pixel
    /// reduces coverage near its position but doesn't contaminate values.
    /// Interior pixels far from the bad pixel should still be ~4.0.
    #[test]
    fn test_pixel_weight_with_gaussian_kernel() {
        let image = AstroImage::from_pixels(ImageDimensions::new(12, 12, 1), vec![4.0; 12 * 12]);

        let mut pw = Buffer2::new_filled(12, 12, 1.0f32);
        *pw.get_mut(5, 5) = 0.0;

        let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(12, 12, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
        let result = acc.finalize();
        let out = result.image.channel(0);
        let w = 24usize;

        // Far from bad pixel: should be ~4.0
        let far = out[2 * w + 2];
        assert!(
            (far - 4.0).abs() < 0.05,
            "Far pixel should be ~4.0, got {}",
            far
        );

        // Another far pixel
        let far2 = out[20 * w + 20];
        assert!(
            (far2 - 4.0).abs() < 0.05,
            "Far pixel should be ~4.0, got {}",
            far2
        );
    }

    /// Test that pixel_weights with wrong dimensions panics.
    #[test]
    #[should_panic(expected = "Pixel weight map dimensions")]
    fn test_pixel_weight_dimensions_mismatch_panics() {
        let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 16]);
        let pw = Buffer2::new_filled(3, 3, 1.0f32); // Wrong size!

        let config = DrizzleConfig::x2();
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
        acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    }
}
