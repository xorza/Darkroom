//! What a drizzle run reconstructs onto, and with which kernel.

use std::path::Path;

use crate::combine::stack_config::StackConfig;
use crate::config_field::ConfigField;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::drizzle_kernel::DrizzleKernel;
use crate::drizzle::drizzle_run::DrizzleRun;
use crate::drizzle::error::{DrizzleConfigError, DrizzleError};
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::cfa_decoder::CfaDecoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::stack_product::StackProduct;

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
    /// Validate parameters before allocating or processing an output image.
    pub(crate) fn validate(&self) -> Result<(), DrizzleConfigError> {
        ConfigField::new("scale", self.scale)
            .finite_and("finite and positive", |value| value > 0.0)?;
        ConfigField::new("pixfrac", self.pixfrac)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)?;
        ConfigField::new("fill_value", self.fill_value).finite()?;
        ConfigField::new("min_weight_fraction", self.min_weight_fraction)
            .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))?;
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
    pub fn drizzle_paths<P: AsRef<Path>>(
        &self,
        frames: Vec<DrizzleFrame<P>>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, DrizzleError> {
        self.begin(ingest, monitor)
            .drizzle_files::<LinearDecoder, P>(frames)
    }

    /// Drizzle frames already held in memory, and combine them under [`Self::stack`].
    ///
    /// In-memory counterpart to [`Self::drizzle_paths`]: skips the per-frame disk load when the
    /// caller already owns the decoded frames. The frames are consumed.
    ///
    /// # Errors
    ///
    /// As [`Self::drizzle_paths`], but for the loads.
    pub fn drizzle_images(
        &self,
        frames: Vec<DrizzleFrame<LinearImage>>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, DrizzleError> {
        self.begin(ingest, monitor).drizzle_held(frames)
    }

    /// Drizzle calibrated, un-demosaiced sensor frames from disk — mosaic FITS or camera RAW — and
    /// combine them under [`Self::stack`]: CFA (Bayer or X-Trans) drizzle.
    ///
    /// Each photosite deposits into the output channel of its color alone, so a color plane holds
    /// only that color's own samples, and an output pixel no photosite of a color reached is left
    /// out of that channel's combine. That takes many well-dithered frames to fill every color at
    /// every pixel; a few undithered frames are better demosaiced and stacked. Register the frames
    /// on [`CfaImage::green_proxy`], which interpolates nothing but the green the stars are
    /// measured on.
    ///
    /// A normalized combine compares each frame with its reference over the pixels both reached in
    /// a color. At scale 1 a sub-pixel dither shares them all; above it, two frames a photosite
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
    pub fn drizzle_cfa_paths<P: AsRef<Path>>(
        &self,
        frames: Vec<DrizzleFrame<P>>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, DrizzleError> {
        self.begin(ingest, monitor)
            .drizzle_files::<CfaDecoder, P>(frames)
    }

    /// Drizzle calibrated mosaics already held in memory, and combine them under [`Self::stack`]:
    /// the in-memory counterpart to [`Self::drizzle_cfa_paths`]. The frames are consumed.
    ///
    /// # Errors
    ///
    /// As [`Self::drizzle_cfa_paths`], but for the loads.
    pub fn drizzle_cfa_images(
        &self,
        frames: Vec<DrizzleFrame<CfaImage>>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, DrizzleError> {
        self.begin(ingest, monitor).drizzle_held(frames)
    }

    /// A drizzle under this configuration, taking its memory reading and decode policy from
    /// `ingest`, and its progress and cancel token from `monitor`, now.
    fn begin(&self, ingest: &IngestConfig, monitor: ProgressMonitor) -> DrizzleRun<'_> {
        DrizzleRun {
            config: self,
            ingest: IngestRun::new(ingest, monitor),
        }
    }
}
