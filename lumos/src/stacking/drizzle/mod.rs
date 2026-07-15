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

pub mod error;

use std::path::Path;

use arrayvec::ArrayVec;
use glam::DVec2;
use rayon::prelude::*;

use crate::ImageDimensions;
use crate::io::astro_image::AstroImage;
use crate::stacking::combine::progress::report_progress;
use crate::stacking::combine::progress::{ProgressCallback, StackingStage};
use crate::stacking::product::StackProduct;
use crate::stacking::registration::transform::Transform;
use error::{DrizzleConfigError, DrizzleError};
use imaginarium::Buffer2;

/// Maximum number of channels (RGB = 3).
const MAX_CHANNELS: usize = 3;

/// Minimum Jacobian (area magnification) to accept a pixel mapping.
/// Below this, the transform is degenerate (e.g. maps to a line or point).
const JACOBIAN_MIN: f64 = 1e-30;

/// Minimum total kernel weight for radial kernels (Gaussian, Lanczos).
/// If the sum of kernel weights is below this, the pixel is skipped
/// to avoid division by near-zero.
const KERNEL_WEIGHT_MIN: f32 = 1e-10;

/// Threshold for treating `sin(pi*x)/(pi*x)` as 1.0 in Lanczos kernel.
/// When |x| < this value, the sinc function is indistinguishable from 1.0.
const SINC_ZERO_THRESHOLD: f32 = 1e-6;

/// Minimum |dx| in `sgarea()` to compute a meaningful slope.
/// Below this the line segment is effectively vertical and contributes
/// negligible area.
const SGAREA_DX_MIN: f64 = 1e-14;

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
        self.min_coverage = min_coverage;
        self
    }

    /// Validate parameters before allocating or processing an output image.
    pub fn validate(&self) -> Result<(), DrizzleConfigError> {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return Err(DrizzleConfigError::InvalidScale { value: self.scale });
        }
        if !self.pixfrac.is_finite() || !(0.0..=1.0).contains(&self.pixfrac) {
            return Err(DrizzleConfigError::InvalidPixfrac {
                value: self.pixfrac,
            });
        }
        if !self.fill_value.is_finite() {
            return Err(DrizzleConfigError::InvalidFillValue {
                value: self.fill_value,
            });
        }
        if !self.min_coverage.is_finite() || !(0.0..=1.0).contains(&self.min_coverage) {
            return Err(DrizzleConfigError::InvalidMinCoverage {
                value: self.min_coverage,
            });
        }
        if self.kernel == DrizzleKernel::Lanczos && (self.scale != 1.0 || self.pixfrac != 1.0) {
            return Err(DrizzleConfigError::InvalidLanczosSampling {
                scale: self.scale,
                pixfrac: self.pixfrac,
            });
        }
        Ok(())
    }
}

/// One drizzle input and all metadata that must remain aligned with it.
#[derive(Debug, Clone)]
pub struct DrizzleFrame<T> {
    /// Image or path to load.
    pub source: T,
    /// Registration transform from input coordinates to the common reference grid.
    pub transform: Transform,
    /// Non-negative per-frame quality weight.
    pub weight: f32,
    /// Optional non-negative per-pixel quality weights with the same dimensions as the image.
    pub pixel_weight_map: Option<Buffer2<f32>>,
}

impl<T> DrizzleFrame<T> {
    /// Create an equally weighted frame without a per-pixel weight map.
    pub fn new(source: T, transform: Transform) -> Self {
        Self {
            source,
            transform,
            weight: 1.0,
            pixel_weight_map: None,
        }
    }
}

/// Drizzle accumulator for building the output image.
#[derive(Debug)]
pub struct DrizzleAccumulator {
    input_dims: ImageDimensions,
    frames_added: usize,
    /// Accumulated weighted flux values (`Σ fluxᵢ·wᵢ`), one Buffer2 per channel.
    data: ArrayVec<Buffer2<f32>, MAX_CHANNELS>,
    /// Accumulated drizzle weight `Σ wᵢ` per output pixel. Channel-independent (the per-pixel
    /// `wᵢ` is purely geometric × frame weight), so a single map serves all channels.
    weight: Buffer2<f32>,
    /// Accumulated squared weight `Σ wᵢ²` per output pixel — drives the output variance map
    /// (`Var = Σwᵢ²/(Σwᵢ)²` per unit input variance), which the correlation-suppressed image RMS
    /// understates.
    weight_sq: Buffer2<f32>,
    /// Configuration.
    config: DrizzleConfig,
}

impl DrizzleAccumulator {
    /// Create a new drizzle accumulator for the given input dimensions.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` is invalid or the input dimensions are empty or have a
    /// channel count other than one or three.
    pub fn new(input_dims: ImageDimensions, config: DrizzleConfig) -> Result<Self, DrizzleError> {
        config.validate()?;
        if input_dims.size.x == 0 || input_dims.size.y == 0 || !matches!(input_dims.channels, 1 | 3)
        {
            return Err(DrizzleError::InvalidInputDimensions {
                width: input_dims.size.x,
                height: input_dims.size.y,
                channels: input_dims.channels,
            });
        }
        let output_width = (input_dims.size.x as f32 * config.scale).ceil() as usize;
        let output_height = (input_dims.size.y as f32 * config.scale).ceil() as usize;

        let mut data = ArrayVec::new();
        for _ in 0..input_dims.channels {
            data.push(Buffer2::new_default(output_width, output_height));
        }

        Ok(Self {
            input_dims,
            frames_added: 0,
            data,
            weight: Buffer2::new_default(output_width, output_height),
            weight_sq: Buffer2::new_default(output_width, output_height),
            config,
        })
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
        ImageDimensions::new((self.width(), self.height()), self.channels())
    }

    /// Validate and add one coherent frame to the accumulator.
    ///
    /// # Errors
    ///
    /// Returns an error when image dimensions differ from the accumulator, or when frame or pixel
    /// weights are negative or non-finite. The accumulator is unchanged on error.
    pub fn add_frame(&mut self, frame: DrizzleFrame<AstroImage>) -> Result<(), DrizzleError> {
        let index = self.frames_added;
        if frame.source.dimensions != self.input_dims {
            return Err(DrizzleError::DimensionMismatch {
                index,
                expected: self.input_dims,
                actual: frame.source.dimensions,
            });
        }
        if !frame.weight.is_finite() || frame.weight < 0.0 {
            return Err(DrizzleError::InvalidFrameWeight {
                index,
                value: frame.weight,
            });
        }
        if let Some(pixel_weights) = &frame.pixel_weight_map {
            if (pixel_weights.width(), pixel_weights.height())
                != (self.input_dims.size.x, self.input_dims.size.y)
            {
                return Err(DrizzleError::PixelWeightDimensionMismatch {
                    index,
                    expected_width: self.input_dims.size.x,
                    expected_height: self.input_dims.size.y,
                    actual_width: pixel_weights.width(),
                    actual_height: pixel_weights.height(),
                });
            }
            if let Some((pixel_index, &value)) = pixel_weights
                .pixels()
                .iter()
                .enumerate()
                .find(|(_, value)| !value.is_finite() || **value < 0.0)
            {
                return Err(DrizzleError::InvalidPixelWeight {
                    frame_index: index,
                    pixel_index,
                    value,
                });
            }
        }

        self.add_image(
            frame.source,
            &frame.transform,
            frame.weight,
            frame.pixel_weight_map.as_ref(),
        );
        self.frames_added += 1;
        Ok(())
    }

    fn add_image(
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

                // Integer-center throughout: input pixel `i` is at coordinate `i` (matching
                // star centroids / `register` / `warp`), and output pixel `o` is the cell
                // `[o - 0.5, o + 0.5)`. The drop center needs no coordinate adjustment.
                let t = transform.apply(DVec2::new(ix as f64, iy as f64));
                let ox_center = t.x as f32 * scale;
                let oy_center = t.y as f32 * scale;

                let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }

                // Output pixel `o` is the cell `[o - 0.5, o + 0.5)`, so the drop touches the
                // pixels `round(min) ..= round(max)` (the `overlap > 0.0` test below drops any
                // boundary cell that doesn't actually touch).
                let ox_min = (ox_center - half_drop).round().max(0.0) as usize;
                let oy_min = (oy_center - half_drop).round().max(0.0) as usize;
                let ox_max =
                    ((ox_center + half_drop).round() + 1.0).min(output_width as f32) as usize;
                let oy_max =
                    ((oy_center + half_drop).round() + 1.0).min(output_height as f32) as usize;

                let effective_weight = weight * pw / jaco;
                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
                        let overlap = compute_square_overlap(
                            ox_center - half_drop,
                            oy_center - half_drop,
                            ox_center + half_drop,
                            oy_center + half_drop,
                            ox as f32 - 0.5,
                            oy as f32 - 0.5,
                            ox as f32 + 0.5,
                            oy as f32 + 0.5,
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

                // Compute 4 corners of the shrunken drop in input space. Input pixel (ix, iy)
                // is integer-center (center at (ix, iy), matching centroids / warp).
                // Winding order: BL, BR, TR, TL (counterclockwise).
                let cx = ix as f64;
                let cy = iy as f64;
                let corners_in = [
                    DVec2::new(cx - dh, cy - dh),
                    DVec2::new(cx + dh, cy - dh),
                    DVec2::new(cx + dh, cy + dh),
                    DVec2::new(cx - dh, cy + dh),
                ];

                // Transform the 4 corners to integer-center output coordinates (output pixel
                // `o` is centered at `o`); `boxer` is given each cell as `[o - 0.5, o + 0.5)`.
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
                if abs_jaco < JACOBIAN_MIN {
                    continue; // Degenerate quadrilateral
                }

                // Bounding box of the output quadrilateral
                let xmin = xout.iter().copied().fold(f64::INFINITY, f64::min);
                let xmax = xout.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let ymin = yout.iter().copied().fold(f64::INFINITY, f64::min);
                let ymax = yout.iter().copied().fold(f64::NEG_INFINITY, f64::max);

                // Output pixel `o` is the cell `[o - 0.5, o + 0.5)`, so the quad bbox touches
                // pixels `round(min) ..= round(max)`.
                let ox_min = xmin.round().max(0.0) as usize;
                let oy_min = ymin.round().max(0.0) as usize;
                let ox_max = (xmax.round() + 1.0).min(output_width as f64) as usize;
                let oy_max = (ymax.round() + 1.0).min(output_height as f64) as usize;

                let effective_weight = weight as f64 * pw as f64;
                let w_over_jaco = effective_weight / abs_jaco;

                for oy in oy_min..oy_max {
                    for ox in ox_min..ox_max {
                        let overlap = boxer(ox as f64 - 0.5, oy as f64 - 0.5, &xout, &yout);
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

                // Integer-center input; flux lands in the nearest output pixel.
                let t = transform.apply(DVec2::new(ix as f64, iy as f64));
                let ox = (t.x as f32 * scale).round() as isize;
                let oy = (t.y as f32 * scale).round() as isize;

                if ox >= 0 && ox < output_width as isize && oy >= 0 && oy < output_height as isize {
                    let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                    if jaco < JACOBIAN_MIN as f32 {
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

                // Integer-center: output pixel `o` is centred at `o`, so the kernel distance
                // is `o - ox_center` with no offset.
                let t = transform.apply(DVec2::new(ix as f64, iy as f64));
                let ox_center = t.x as f32 * scale;
                let oy_center = t.y as f32 * scale;

                let jaco = local_jacobian(transform, t, ix, iy, scale as f64) as f32;
                if jaco < JACOBIAN_MIN as f32 {
                    continue;
                }

                let ox_int = ox_center.round() as isize;
                let oy_int = oy_center.round() as isize;

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
                        let dist_x = ox as f32 - ox_center;
                        let dist_y = oy as f32 - oy_center;
                        total_weight += kernel_fn(dist_x, dist_y);
                    }
                }

                if total_weight.abs() < KERNEL_WEIGHT_MIN {
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
                        let dist_x = ox as f32 - ox_center;
                        let dist_y = oy as f32 - oy_center;
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
        for (c, d) in self.data.iter_mut().enumerate() {
            let flux = image.channel(c)[(ix, iy)];
            *d.get_mut(ox, oy) += flux * pixel_weight;
        }
        // Weight is channel-independent, so accumulate it (and its square, for the variance map)
        // once per output pixel rather than once per channel.
        *self.weight.get_mut(ox, oy) += pixel_weight;
        *self.weight_sq.get_mut(ox, oy) += pixel_weight * pixel_weight;
    }

    /// Finalize the drizzle result: normalize flux by weight and emit the coverage, weight, and
    /// variance maps. The weight `Σwᵢ` is channel-independent (shared geometric overlap), so one
    /// map normalizes every channel and seeds the coverage/weight/variance outputs.
    pub fn finalize(self) -> StackProduct {
        let width = self.width();
        let height = self.height();
        let n_channels = self.channels();
        let needs_clamping = self.config.kernel == DrizzleKernel::Lanczos;
        let min_coverage = self.config.min_coverage;
        let fill_value = self.config.fill_value;

        let weight_pixels = self.weight.pixels();
        let weight_sq_pixels = self.weight_sq.pixels();

        // Find max weight for normalizing the coverage / min_coverage threshold.
        let max_weight = weight_pixels
            .par_iter()
            .copied()
            .reduce(|| 0.0f32, f32::max);
        let weight_threshold = if max_weight > 0.0 {
            min_coverage * max_weight
        } else {
            0.0
        };

        // Build per-channel output (row-parallel normalization by the shared weight).
        let output_channels: Vec<Vec<f32>> = (0..n_channels)
            .map(|c| {
                let data_pixels = self.data[c].pixels();
                let mut out = vec![fill_value; width * height];

                out.par_chunks_mut(width)
                    .enumerate()
                    .for_each(|(y, out_row)| {
                        let row_start = y * width;
                        for (x, out_val) in out_row.iter_mut().enumerate() {
                            let idx = row_start + x;
                            let w = weight_pixels[idx];
                            if w > 0.0 && w >= weight_threshold {
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

        // Output variance per unit input-pixel variance: Var(O) = Σ(wᵢ²)/(Σwᵢ)². `0` where
        // uncovered. This is the true per-pixel noise that the correlated image RMS understates.
        let mut variance = Buffer2::new_default(width, height);
        variance
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(idx, v)| {
                let w = weight_pixels[idx];
                *v = if w > 0.0 {
                    weight_sq_pixels[idx] / (w * w)
                } else {
                    0.0
                };
            });

        // Normalized coverage [0, 1] for masking.
        let mut coverage = Buffer2::new_default(width, height);
        if max_weight > 0.0 {
            let inv_max = 1.0 / max_weight;
            coverage
                .pixels_mut()
                .par_iter_mut()
                .zip(weight_pixels.par_iter())
                .for_each(|(c, &w)| *c = w * inv_max);
        }

        let image = AstroImage::from_planar_channels(
            ImageDimensions::new((width, height), n_channels),
            output_channels,
        );

        StackProduct {
            image,
            coverage,
            weight: self.weight,
            variance,
        }
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
    // Integer-center: `center` is `T(ix, iy)`; sample the +x and +y neighbours a unit apart.
    let right = transform.apply(DVec2::new(ix as f64 + 1.0, iy as f64));
    let down = transform.apply(DVec2::new(ix as f64, iy as f64 + 1.0));
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
    if x.abs() < SINC_ZERO_THRESHOLD {
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
    if dx.abs() < SGAREA_DX_MIN {
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

/// Compute overlap area between a convex quadrilateral and a pixel cell.
///
/// Shifts the quadrilateral so that the cell with lower-left corner `(ox, oy)` becomes the
/// unit square [0,1]×[0,1], then sums signed areas from each edge via `sgarea()`.
///
/// Port of STScI `boxer()` from cdrizzlebox.c. Output pixels are integer-center (pixel `o`
/// spans `[o - 0.5, o + 0.5]`, matching STScI), so callers pass the cell's lower-left
/// corner `o - 0.5`.
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

fn load_drizzle_frame<P: AsRef<Path>>(
    frame: DrizzleFrame<P>,
) -> Result<DrizzleFrame<AstroImage>, DrizzleError> {
    let DrizzleFrame {
        source,
        transform,
        weight,
        pixel_weight_map,
    } = frame;
    let path = source.as_ref();
    let image = AstroImage::from_file(path).map_err(|error| DrizzleError::ImageLoad {
        path: path.to_path_buf(),
        source: std::io::Error::other(error),
    })?;
    Ok(DrizzleFrame {
        source: image,
        transform,
        weight,
        pixel_weight_map,
    })
}

/// Drizzle stack images from disk with per-frame transforms.
///
/// Streams frames one at a time (only one input image is resident at a time).
/// To drizzle frames already held in memory, use [`drizzle_images`].
///
/// # Arguments
///
/// * `frames` - Paths bundled with their transform and optional quality weights
/// * `config` - Drizzle configuration
/// * `progress` - Progress callback
///
/// # Returns
///
/// The drizzled result: image plus coverage, weight (`Σwᵢ`), and variance (`Σwᵢ²/(Σwᵢ)²`) maps.
///
/// # Errors
///
/// Returns an error for invalid configuration, missing frames, image loading failures,
/// inconsistent image dimensions, or invalid frame weights.
pub fn drizzle_stack<P: AsRef<Path>>(
    frames: Vec<DrizzleFrame<P>>,
    config: &DrizzleConfig,
    progress: ProgressCallback,
) -> Result<StackProduct, DrizzleError> {
    if frames.is_empty() {
        return Err(DrizzleError::NoFrames);
    }
    config.validate()?;

    let frame_count = frames.len();
    let mut frames = frames.into_iter();
    let first = load_drizzle_frame(frames.next().unwrap())?;
    let input_dims = first.source.dimensions;

    tracing::info!(
        input_width = input_dims.size.x,
        input_height = input_dims.size.y,
        channels = input_dims.channels,
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        frame_count,
        "Starting drizzle stacking (from paths)"
    );

    let mut accumulator = DrizzleAccumulator::new(input_dims, config.clone())?;
    accumulator.add_frame(first)?;
    report_progress(&progress, 1, frame_count, StackingStage::Processing);

    for (index, frame) in frames.enumerate() {
        accumulator.add_frame(load_drizzle_frame(frame)?)?;
        report_progress(&progress, index + 2, frame_count, StackingStage::Processing);
    }

    Ok(accumulator.finalize())
}

/// Drizzle stack frames already held in memory.
///
/// In-memory counterpart to [`drizzle_stack`]: skips the per-frame disk load when
/// the caller already owns the decoded frames. The frames are consumed.
///
/// # Errors
///
/// Returns an error for invalid configuration, missing frames, inconsistent image dimensions, or
/// invalid frame weights.
pub fn drizzle_images(
    frames: Vec<DrizzleFrame<AstroImage>>,
    config: &DrizzleConfig,
    progress: ProgressCallback,
) -> Result<StackProduct, DrizzleError> {
    if frames.is_empty() {
        return Err(DrizzleError::NoFrames);
    }
    config.validate()?;

    let frame_count = frames.len();
    let input_dims = frames[0].source.dimensions;

    tracing::info!(
        input_width = input_dims.size.x,
        input_height = input_dims.size.y,
        channels = input_dims.channels,
        output_scale = config.scale,
        pixfrac = config.pixfrac,
        kernel = ?config.kernel,
        frame_count,
        "Starting drizzle stacking (in memory)"
    );

    let mut accumulator = DrizzleAccumulator::new(input_dims, config.clone())?;
    for (index, frame) in frames.into_iter().enumerate() {
        accumulator.add_frame(frame)?;
        report_progress(&progress, index + 1, frame_count, StackingStage::Processing);
    }

    Ok(accumulator.finalize())
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod synthetic_tests;
#[cfg(test)]
mod tests;
