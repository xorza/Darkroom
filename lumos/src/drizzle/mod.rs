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
    /// Square kernel: true polygon clipping via Sutherland-Hodgman / Green's theorem.
    /// Transforms all 4 corners of each input pixel drop, computes exact quadrilateral-
    /// to-output-pixel overlap area. Correct for any transform including rotation and shear.
    /// Reference: STScI cdrizzlebox.c `do_kernel_square` / `boxer` / `sgarea`.
    Square,
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
            DrizzleKernel::Square => {
                self.add_image_square(&image, transform, weight, pixel_weights, scale);
            }
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

                let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                if jaco < 1e-30 {
                    continue;
                }

                let ox_min = (ox_center - half_drop).floor().max(0.0) as usize;
                let oy_min = (oy_center - half_drop).floor().max(0.0) as usize;
                let ox_max = (ox_center + half_drop).ceil().min(output_width as f32) as usize;
                let oy_max = (oy_center + half_drop).ceil().min(output_height as f32) as usize;

                let effective_weight = weight * pw / jaco;
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

    /// Add image using square kernel (true polygon clipping).
    ///
    /// For each input pixel, transforms all 4 corners of the (pixfrac-shrunken) drop
    /// to output coordinates, computes the Jacobian (signed area of the output
    /// quadrilateral), then iterates output pixels in the bounding box and computes
    /// exact overlap via `boxer()`.
    ///
    /// Reference: STScI cdrizzlebox.c `do_kernel_square`.
    fn add_image_square(
        &mut self,
        image: &AstroImage,
        transform: &Transform,
        weight: f32,
        pixel_weights: Option<&Buffer2<f32>>,
        scale: f32,
    ) {
        let pixfrac = self.config.pixfrac;
        let dh = 0.5 * pixfrac as f64;
        let scale_f64 = scale as f64;
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

                // Compute 4 corners of the shrunken drop in input space.
                // Input pixel (ix, iy) has center at (ix+0.5, iy+0.5).
                // Winding order: BL, BR, TR, TL (counterclockwise).
                let cx = ix as f64 + 0.5;
                let cy = iy as f64 + 0.5;
                let corners_in = [
                    DVec2::new(cx - dh, cy - dh),
                    DVec2::new(cx + dh, cy - dh),
                    DVec2::new(cx + dh, cy + dh),
                    DVec2::new(cx - dh, cy + dh),
                ];

                // Transform all 4 corners to output coordinates and scale
                let mut xout = [0.0f64; 4];
                let mut yout = [0.0f64; 4];
                for (k, corner) in corners_in.iter().enumerate() {
                    let t = transform.apply(*corner);
                    xout[k] = t.x * scale_f64;
                    yout[k] = t.y * scale_f64;
                }

                // Jacobian: signed area of the output quadrilateral via diagonal cross product
                let jaco = 0.5
                    * ((xout[1] - xout[3]) * (yout[0] - yout[2])
                        - (xout[0] - xout[2]) * (yout[1] - yout[3]));
                let abs_jaco = jaco.abs();
                if abs_jaco < 1e-30 {
                    continue; // Degenerate quadrilateral
                }

                // Bounding box of the output quadrilateral
                let xmin = xout.iter().copied().fold(f64::INFINITY, f64::min);
                let xmax = xout.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let ymin = yout.iter().copied().fold(f64::INFINITY, f64::min);
                let ymax = yout.iter().copied().fold(f64::NEG_INFINITY, f64::max);

                let ox_min = (xmin.floor().max(0.0)) as usize;
                let oy_min = (ymin.floor().max(0.0)) as usize;
                let ox_max = (xmax.ceil().min(output_width as f64)) as usize;
                let oy_max = (ymax.ceil().min(output_height as f64)) as usize;

                let effective_weight = weight as f64 * pw as f64;
                let w_over_jaco = effective_weight / abs_jaco;

                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
                        let overlap = boxer(ox as f64, oy as f64, &xout, &yout);
                        if overlap > 0.0 {
                            let pixel_weight = (overlap * w_over_jaco) as f32;
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
                    let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                    if jaco < 1e-30 {
                        continue;
                    }
                    self.accumulate(image, ix, iy, ox as usize, oy as usize, weight * pw / jaco);
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

                let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                if jaco < 1e-30 {
                    continue;
                }

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
                // Jacobian correction: divide by local area magnification.
                let inv_total = (weight * pw) / (total_weight * jaco);
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

/// Compute local Jacobian determinant (area magnification) at pixel `(ix, iy)`.
///
/// Uses finite differences: transforms center, center+dx, center+dy through the
/// transform and computes |det([∂out/∂x, ∂out/∂y])| * scale².
///
/// For affine transforms this is constant (= det(M) * scale²).
/// For homographies it varies spatially.
#[inline]
fn local_jacobian(transform: &Transform, center: DVec2, ix: usize, iy: usize, scale: f64) -> f64 {
    let right = transform.apply(DVec2::new(ix as f64 + 1.5, iy as f64 + 0.5));
    let down = transform.apply(DVec2::new(ix as f64 + 0.5, iy as f64 + 1.5));
    let dx = right - center;
    let dy = down - center;
    (dx.x * dy.y - dx.y * dy.x).abs() * scale * scale
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

/// Compute signed area between a line segment and the x-axis, clipped to the unit square
/// [0,1]×[0,1]. Uses Green's theorem. Port of STScI `sgarea()` from cdrizzlebox.c.
///
/// The sign depends on the direction of traversal (left-to-right = positive).
/// When summed over all 4 edges of a convex quadrilateral (counterclockwise winding),
/// the total gives the overlap area between the quadrilateral and the unit square.
#[inline]
fn sgarea(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;

    // Near-vertical line contributes negligible area
    if dx.abs() < 1e-14 {
        return 0.0;
    }

    // Determine traversal direction and sort x endpoints
    let (sgn_dx, xlo, xhi) = if dx < 0.0 {
        (-1.0, x2, x1)
    } else {
        (1.0, x1, x2)
    };

    // Segment entirely outside unit square horizontally
    if xlo >= 1.0 || xhi <= 0.0 {
        return 0.0;
    }

    // Clip x to [0, 1]
    let xlo = xlo.max(0.0);
    let xhi = xhi.min(1.0);

    // Compute y at clipped x positions via line equation
    let slope = dy / dx;
    let ylo = y1 + slope * (xlo - x1);
    let yhi = y1 + slope * (xhi - x1);

    // Segment entirely below x-axis
    if ylo <= 0.0 && yhi <= 0.0 {
        return 0.0;
    }

    // Segment entirely above y=1: full rectangle contribution
    if ylo >= 1.0 && yhi >= 1.0 {
        return sgn_dx * (xhi - xlo);
    }

    // det = x1*y2 - y1*x2 (used for x-intercept and y=1 intercept)
    let det = x1 * y2 - y1 * x2;

    // Clip to y >= 0 (adjust x endpoint where segment crosses y=0)
    let (xlo, ylo) = if ylo < 0.0 {
        (det / dy, 0.0)
    } else {
        (xlo, ylo)
    };
    let (xhi, yhi) = if yhi < 0.0 {
        (det / dy, 0.0)
    } else {
        (xhi, yhi)
    };

    if ylo <= 1.0 {
        if yhi <= 1.0 {
            // Case A: both y in [0,1] — trapezoid area
            return sgn_dx * 0.5 * (xhi - xlo) * (yhi + ylo);
        }
        // Case B: enters inside, exits above y=1
        // Split at x where y=1: xtop = (dx + det) / dy
        let xtop = (dx + det) / dy;
        return sgn_dx * (0.5 * (xtop - xlo) * (1.0 + ylo) + xhi - xtop);
    }

    // Case C: enters above y=1, exits inside
    let xtop = (dx + det) / dy;
    sgn_dx * (0.5 * (xhi - xtop) * (1.0 + yhi) + xtop - xlo)
}

/// Compute overlap area between a convex quadrilateral and an output pixel.
///
/// Shifts the quadrilateral so that output pixel (ox, oy) becomes the unit square
/// [0,1]×[0,1], then sums signed areas from each edge via `sgarea()`.
///
/// Port of STScI `boxer()` from cdrizzlebox.c. The STScI version uses pixel-centered
/// coordinates (pixel `(i,j)` spans `[i-0.5, i+0.5]`), but our code uses pixel-corner
/// coordinates (pixel `(ox, oy)` spans `[ox, ox+1]`), so we shift by `ox` / `oy`
/// directly (no 0.5 adjustment needed).
#[inline]
fn boxer(ox: f64, oy: f64, x: &[f64; 4], y: &[f64; 4]) -> f64 {
    let px = [x[0] - ox, x[1] - ox, x[2] - ox, x[3] - ox];
    let py = [y[0] - oy, y[1] - oy, y[2] - oy, y[3] - oy];

    let mut sum = 0.0;
    for i in 0..4 {
        let j = (i + 1) & 3;
        sum += sgarea(px[i], py[i], px[j], py[j]);
    }
    sum.abs()
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
        source: std::io::Error::other(e),
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
            source: std::io::Error::other(e),
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
mod tests;
