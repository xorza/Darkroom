//! What a drizzle run reconstructs onto, and with which kernel.

use std::path::Path;

use common::CancelToken;

use crate::combine::stack_config::StackConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::drizzle_result::DrizzleResult;
use crate::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::drizzle::stack;
use crate::error::InvalidConfigField;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::math::size2us::Size2us;
use crate::progress::progress_callback::ProgressCallback;

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
    /// A Lanczos interpolation of the frame at each output pixel. Only valid at pixfrac = 1 and
    /// scale = 1, where it resamples rather than drizzles, and the slowest kernel. Its weights go
    /// negative, so it rings at sharp edges as any Lanczos resampling does.
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
    /// How the drizzled frames combine: [`StackConfig::light`] by default.
    pub stack: StackConfig,
}

impl Default for DrizzleConfig {
    fn default() -> Self {
        Self {
            scale: 2.0,
            pixfrac: 0.8,
            kernel: DrizzleKernel::Square,
            fill_value: 0.0,
            min_weight_fraction: 0.1,
            stack: StackConfig::light(),
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

    /// Drizzle images from disk with per-frame warps, and combine them under [`Self::stack`].
    ///
    /// Streams frames one at a time (only one input image is resident at a time). To drizzle frames
    /// already held in memory, use [`Self::drizzle_images`]; to drizzle un-demosaiced sensor
    /// frames, use [`Self::drizzle_cfa_paths`].
    ///
    /// # Returns
    ///
    /// The drizzled product — its image in the input's surface-brightness units, see
    /// [`Self::scale`] — with the quality planes [`Self::stack`] asks for, and the count of input
    /// pixels a SIP warp could not place.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid configuration, missing frames, image loading failures,
    /// inconsistent image dimensions, a non-finite sample, invalid pixel weights, a failed combine,
    /// or cancellation.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "every stacking entry takes its progress and cancel token the same way, by value"
    )]
    pub fn drizzle_paths<P: AsRef<Path>>(
        &self,
        frames: Vec<DrizzleFrame<P>>,
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<DrizzleResult, DrizzleError> {
        stack::from_files::<LinearImage, P>(frames, self, ingest, &progress, cancel)
    }

    /// Drizzle frames already held in memory, and combine them under [`Self::stack`].
    ///
    /// In-memory counterpart to [`Self::drizzle_paths`]: skips the per-frame disk load when the
    /// caller already owns the decoded frames. The frames are consumed.
    ///
    /// # Errors
    ///
    /// As [`Self::drizzle_paths`], but for the loads.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "every stacking entry takes its progress and cancel token the same way, by value"
    )]
    pub fn drizzle_images(
        &self,
        frames: Vec<DrizzleFrame<LinearImage>>,
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<DrizzleResult, DrizzleError> {
        stack::from_memory(frames, self, ingest, &progress, cancel)
    }

    /// Drizzle calibrated, un-demosaiced sensor frames from disk — mosaic FITS or camera RAW — and
    /// combine them under [`Self::stack`]: CFA (Bayer or X-Trans) drizzle.
    ///
    /// Each photosite deposits into the output channel of its colour alone, so a colour plane holds
    /// only that colour's own samples, and an output pixel no photosite of a colour reached is left
    /// out of that channel's combine. That takes many well-dithered frames to fill every colour at
    /// every pixel; a few undithered frames are better demosaiced and stacked. Register the frames
    /// on [`CfaImage::green_proxy`], which interpolates nothing but the green the stars are
    /// measured on.
    ///
    /// A normalized combine compares each frame with its reference over the pixels both reached in
    /// a colour. At scale 1 a sub-pixel dither shares them all; above it, two frames a photosite
    /// apart can share none, and the combine then fails with
    /// [`StackError::NoCommonCoverage`](crate::combine::error::StackError::NoCommonCoverage).
    /// `Normalization::None` takes any set.
    ///
    /// A RAW file is decoded as it stands: the drizzle calibrates nothing. Calibrate the frames
    /// first, and drizzle the calibrated mosaics, through [`Self::drizzle_cfa_images`] or as FITS
    /// files.
    ///
    /// # Errors
    ///
    /// As [`Self::drizzle_paths`], and for a frame whose mosaic pattern differs from the first
    /// frame's.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "every stacking entry takes its progress and cancel token the same way, by value"
    )]
    pub fn drizzle_cfa_paths<P: AsRef<Path>>(
        &self,
        frames: Vec<DrizzleFrame<P>>,
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<DrizzleResult, DrizzleError> {
        stack::from_files::<CfaImage, P>(frames, self, ingest, &progress, cancel)
    }

    /// Drizzle calibrated mosaics already held in memory, and combine them under [`Self::stack`]:
    /// the in-memory counterpart to [`Self::drizzle_cfa_paths`]. The frames are consumed.
    ///
    /// # Errors
    ///
    /// As [`Self::drizzle_cfa_paths`], but for the loads.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "every stacking entry takes its progress and cancel token the same way, by value"
    )]
    pub fn drizzle_cfa_images(
        &self,
        frames: Vec<DrizzleFrame<CfaImage>>,
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<DrizzleResult, DrizzleError> {
        stack::from_memory(frames, self, ingest, &progress, cancel)
    }
}
