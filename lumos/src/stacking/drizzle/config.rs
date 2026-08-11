//! What a drizzle run reconstructs onto, and with which kernel.

use crate::error::InvalidConfigField;
use crate::stacking::drizzle::error::DrizzleConfigError;
use crate::stacking::stack_product::quality_planes::QualityPlanes;

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
    /// Range: greater than 0.0 and at most 1.0
    /// - 1.0 = shift-and-add (full pixel footprint)
    /// - 0.8 = recommended for 4-point dithered data
    /// - 0.5 = aggressive shrinking, needs good dithering
    pub pixfrac: f32,
    /// Kernel type for flux distribution.
    pub kernel: DrizzleKernel,
    /// Fill value for pixels with no coverage.
    pub fill_value: f32,
    /// Fill gate, as a share of the deepest pixel's accumulated weight (0.0-1.0). A pixel holding
    /// less than this fraction of `max(Σwᵢ)` is set to `fill_value`.
    ///
    /// Not a threshold on [`StackProduct::coverage`](crate::StackProduct::coverage), which reports
    /// the share of *frames* that reached a pixel. This gate asks how much signal landed there,
    /// which dithering geometry varies independently of how many frames contributed.
    pub min_weight_fraction: f32,
    /// Which ancillary planes the drizzle should produce, as [`StackConfig::quality`] asks of a
    /// statistical combine.
    ///
    /// Each is an output-grid plane, and a drizzle's output grid is `scale²` times its input — so
    /// declining one here saves more than it does on the statistical side. `variance` declines the
    /// most: its `Σwᵢ²` accumulator is resident for the whole run, where coverage and weight are
    /// shaped from the weight map that normalizing the image needs anyway.
    ///
    /// [`StackConfig::quality`]: crate::StackConfig::quality
    pub quality: QualityPlanes,
}

impl Default for DrizzleConfig {
    fn default() -> Self {
        Self {
            scale: 2.0,
            pixfrac: 0.8,
            kernel: DrizzleKernel::Turbo,
            fill_value: 0.0,
            min_weight_fraction: 0.1,
            quality: QualityPlanes::ALL,
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
            pixfrac: 0.7,
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
    pub fn with_min_weight_fraction(mut self, min_weight_fraction: f32) -> Self {
        self.min_weight_fraction = min_weight_fraction;
        self
    }

    /// Validate parameters before allocating or processing an output image.
    pub fn validate(&self) -> Result<(), DrizzleConfigError> {
        InvalidConfigField::finite("scale", "finite and positive", self.scale, |value| {
            value > 0.0
        })?;
        InvalidConfigField::finite("pixfrac", "finite and in (0, 1]", self.pixfrac, |value| {
            value > 0.0 && value <= 1.0
        })?;
        InvalidConfigField::finite_only("fill_value", self.fill_value)?;
        InvalidConfigField::finite(
            "min_weight_fraction",
            "finite and in [0, 1]",
            self.min_weight_fraction,
            |value| (0.0..=1.0).contains(&value),
        )?;
        if self.kernel == DrizzleKernel::Lanczos && (self.scale != 1.0 || self.pixfrac != 1.0) {
            return Err(DrizzleConfigError::InvalidLanczosSampling {
                scale: self.scale,
                pixfrac: self.pixfrac,
            });
        }
        Ok(())
    }
}
