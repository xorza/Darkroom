//! What a drizzle run reconstructs onto, and with which kernel.

use crate::drizzle::error::DrizzleConfigError;
use crate::error::InvalidConfigField;
use crate::ingest::ingest_config::IngestConfig;
use crate::math::size2us::Size2us;

/// Drizzle kernel type for distributing flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrizzleKernel {
    /// Square kernel: the exact overlap area of each drop's transformed quadrilateral with every
    /// output pixel, by STScI's `boxer` / `sgarea` edge integration. Correct for any transform,
    /// rotation and shear included. Default, as in DrizzlePac, PixInsight and Siril.
    /// Reference: `STScI` cdrizzlebox.c `do_kernel_square` / `boxer` / `sgarea`.
    #[default]
    Square,
    /// Turbo kernel: axis-aligned rectangular drop centered on the transformed pixel center.
    /// Approximation of true square kernel — always aligned with output X/Y axes regardless
    /// of rotation, so it is exact only at rotations of 0°, 90° and 180°.
    /// (Named "turbo" in `STScI` `DrizzlePac`; "square" there integrates the transformed polygon's
    /// edges, as [`Self::Square`] does.)
    Turbo,
    /// Point kernel - single pixel contribution.
    /// Fastest but requires very good dithering.
    Point,
    /// Gaussian droplet whose FWHM is the drop size, `pixfrac·scale` output pixels, as in
    /// `STScI` drizzle. Smoother output, slight flux redistribution.
    Gaussian,
    /// Lanczos kernel for high-quality interpolation.
    /// Best quality but slowest. Only valid at pixfrac=1.0, scale=1.0.
    Lanczos,
}

impl DrizzleKernel {
    /// Every kernel.
    pub const ALL: [Self; 5] = [
        Self::Square,
        Self::Turbo,
        Self::Point,
        Self::Gaussian,
        Self::Lanczos,
    ];
}

/// Configuration for Drizzle stacking.
#[derive(Debug, Clone)]
pub struct DrizzleConfig {
    /// Output scale factor relative to input (e.g., 2.0 = 2x resolution).
    /// Common values: 1.5, 2.0, 3.0
    ///
    /// The output keeps the input's surface brightness, value per input-pixel area: a flat field
    /// of 1 drizzles to 1 at any scale, so the image total is `scale²` times the input's. Divide by
    /// `scale²` for flux per output pixel, DrizzlePac's convention.
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
    /// How [`drizzle_stack`](crate::drizzle_stack) reads its frames, and where a run whose
    /// drizzled frames do not fit in memory spills them.
    pub ingest: IngestConfig,
}

impl Default for DrizzleConfig {
    fn default() -> Self {
        Self {
            scale: 2.0,
            pixfrac: 0.8,
            kernel: DrizzleKernel::Square,
            fill_value: 0.0,
            min_weight_fraction: 0.1,
            ingest: IngestConfig::default(),
        }
    }
}

impl DrizzleConfig {
    /// The output grid an `input` frame drizzles onto: each side `scale` times the input's, rounded
    /// up so every drop of a validated config lands inside it.
    #[expect(
        clippy::cast_sign_loss,
        reason = "validate holds the scale positive, so each output side is non-negative"
    )]
    pub(crate) fn output_size(&self, input: Size2us) -> Size2us {
        Size2us::new(
            (input.width as f32 * self.scale).ceil() as usize,
            (input.height as f32 * self.scale).ceil() as usize,
        )
    }

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
    #[must_use]
    pub const fn with_pixfrac(mut self, pixfrac: f32) -> Self {
        self.pixfrac = pixfrac;
        self
    }

    /// Set kernel type.
    #[must_use]
    pub const fn with_kernel(mut self, kernel: DrizzleKernel) -> Self {
        self.kernel = kernel;
        self
    }

    /// Set the fill gate, [`Self::min_weight_fraction`].
    #[must_use]
    pub const fn with_min_weight_fraction(mut self, min_weight_fraction: f32) -> Self {
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
