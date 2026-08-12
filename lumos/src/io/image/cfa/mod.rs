//! Raw CFA (Color Filter Array) image representation.
//!
//! Represents sensor data before demosaicing - a single channel with color
//! filter pattern metadata. Used for calibration frame processing (darks,
//! flats, bias) and hot pixel correction on raw data.

pub(crate) mod same_color;

use std::path::Path;

use rayon::prelude::*;

use crate::io::image::cfa::same_color::SameColorMedian;
use crate::io::image::error::ImageError;
use crate::io::image::fits::{cfa as fits_cfa, decode as fits_decode};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{ColorProvenance, DemosaicProvenance};
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::io::image::standard::{
    FITS_EXTENSIONS, STANDARD_IMAGE_EXTENSIONS, file_extension, scientific_rejection,
};
use crate::io::raw;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::{DemosaicError, DemosaicKind};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::frame_store::StackableImage;
use common::CancelToken;
use imaginarium::Buffer2;

/// Standard deviation of uniform error spanning one ADC step: `1 / √12`.
pub(crate) const QUANTIZATION_SIGMA_PER_STEP: f32 = 0.288_675_13;

/// CFA pattern anchored at the origin of the image data it accompanies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfaType {
    /// No CFA pattern (monochrome sensor)
    Mono,
    /// 2x2 Bayer pattern
    Bayer(CfaPattern),
    /// 6x6 X-Trans pattern
    XTrans([[u8; 6]; 6]),
}

impl CfaType {
    /// Get the color index (0=R, 1=G, 2=B) at position (x, y).
    /// For Mono, always returns 0.
    #[inline(always)]
    pub fn color_at(&self, pos: Vec2us) -> u8 {
        match self {
            CfaType::Mono => 0,
            CfaType::Bayer(p) => p.color_at(pos) as u8,
            CfaType::XTrans(pattern) => pattern[pos.y % 6][pos.x % 6],
        }
    }

    /// Number of distinct color channels (1 for Mono, 3 for Bayer/X-Trans).
    pub fn num_colors(&self) -> usize {
        match self {
            CfaType::Mono => 1,
            CfaType::Bayer(_) | CfaType::XTrans(_) => 3,
        }
    }

    /// The pattern to treat a frame as carrying: its own, or Mono when it declares none.
    ///
    /// A frame without CFA metadata is a single-colour mosaic, and [`CfaType::Mono`] describes that
    /// exactly — `color_at` answers 0 everywhere, its period is 1, and every pixel is its own
    /// colour's neighbour. Resolving the `Option` here once is what keeps "absent means mono" out of
    /// the per-pixel colour lookup, the phase-period match, and the same-colour neighbour strategy,
    /// each of which used to spell it out for itself.
    pub(crate) fn or_mono(cfa_type: Option<&Self>) -> &Self {
        cfa_type.unwrap_or(&CfaType::Mono)
    }

    pub(crate) fn demosaic_kind(&self) -> DemosaicKind {
        match self {
            Self::Mono => DemosaicKind::Mono,
            Self::Bayer(_) => DemosaicKind::BayerRcd,
            Self::XTrans(_) => DemosaicKind::XTransMarkesteijn,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CfaFrameInfo {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) demosaic: DemosaicKind,
}

impl CfaFrameInfo {
    pub(crate) fn from_file(path: &Path, context: &LoadContext) -> Result<Self, ImageError> {
        let extension = file_extension(path);
        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            fits_decode::fits_cfa_frame_info(path, context)
        } else if raw::RAW_EXTENSIONS.contains(&extension.as_str()) {
            raw::raw_cfa_frame_info(path, &context.cancel)
        } else {
            Err(scientific_rejection(
                path,
                "scientific CFA input must be camera RAW or FITS",
            ))
        }
    }
}

/// Raw CFA image - single channel with color filter pattern metadata.
/// Represents sensor data before demosaicing.
#[derive(Debug, Clone)]
pub struct CfaImage {
    /// Single-channel linear samples; calibration may put values outside `[0, 1]`.
    /// Layout: row-major, width * height pixels.
    pub data: Buffer2<f32>,
    pub metadata: ImageMetadata,
    /// Source-quantization uncertainty in the current CFA sample units.
    pub(crate) quantization_sigma: Option<f32>,
    /// Which pixels carry no measurement, for a source that declared any. The samples at those
    /// positions are a finite fill, not data — see [`NullMask`].
    pub(crate) nulls: Option<NullMask>,
}

impl StackableImage for CfaImage {
    fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new((self.data.width(), self.data.height()), 1)
    }

    fn nulls(&self) -> Option<&NullMask> {
        self.nulls.as_ref()
    }

    fn channel(&self, c: usize) -> &[f32] {
        assert!(c == 0, "CfaImage has only 1 channel, got {c}");
        &self.data
    }

    fn metadata(&self) -> &ImageMetadata {
        &self.metadata
    }

    fn quantization_sigma(&self) -> Option<f32> {
        self.quantization_sigma
    }

    fn load(path: &std::path::Path, context: &LoadContext) -> Result<Self, ImageError> {
        CfaImage::from_file(path, context)
    }

    fn peek_dimensions(path: &std::path::Path, context: &LoadContext) -> Option<ImageDimensions> {
        CfaFrameInfo::from_file(path, context)
            .ok()
            .map(|info| info.dimensions)
    }

    fn into_planes(self) -> arrayvec::ArrayVec<imaginarium::Buffer2<f32>, 3> {
        let mut planes = arrayvec::ArrayVec::new();
        planes.push(self.data);
        planes
    }
}

impl CfaImage {
    /// Create an in-memory sensor image whose CFA classification is supplied by the caller.
    pub fn from_plane(data: Buffer2<f32>, metadata: ImageMetadata) -> Self {
        assert!(
            metadata.cfa_type.is_some(),
            "CfaImage metadata must identify a monochrome or CFA sensor"
        );
        Self {
            data,
            metadata,
            quantization_sigma: None,
            nulls: None,
        }
    }

    /// Load an un-demosaiced sensor image from camera RAW or mosaic FITS.
    pub fn from_file<P: AsRef<Path>>(path: P, context: &LoadContext) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        let extension = file_extension(path);

        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            return fits_decode::load_cfa_fits(path, context);
        }
        if raw::RAW_EXTENSIONS.contains(&extension.as_str()) {
            return raw::load_raw_cfa(path, &context.cancel);
        }
        if STANDARD_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            return Err(scientific_rejection(
                path,
                "generic raster decoders do not establish a scientific CFA contract",
            ));
        }

        Err(ImageError::UnsupportedFormat { extension })
    }

    /// Resident RAM held by this frame: its single f32 CFA plane's pixel bytes.
    /// Metadata is negligible against a full-sensor plane.
    pub fn ram_bytes(&self) -> usize {
        self.data.width() * self.data.height() * std::mem::size_of::<f32>()
    }

    /// Save this sensor-domain image as a checksummed floating-point FITS file.
    pub fn save_fits(&self, path: &Path) -> std::io::Result<()> {
        fits_cfa::save_cfa_fits(path, self)
    }

    /// Replace every null with the median of its same-colour neighbours.
    ///
    /// The demosaic reads a neighbourhood, so whatever sits under a null reaches output pixels the
    /// mask does not cover. The resampler answers the same problem by dividing the warped image by
    /// the warped validity plane — `resample::masked_warp` — but an adaptive kernel like RCD or
    /// Markesteijn offers no such plane to divide by. Leaving the decoder's frame-median fill there
    /// would spread a value with no local meaning; a same-colour neighbour median spreads a
    /// plausible one, so what escapes the mask is interpolation error rather than fabrication.
    ///
    /// The same repair [`DefectMap`](crate::DefectMap) applies to hot and cold pixels, for the same
    /// reason and through the same neighbour search — mask included, so a cluster of nulls is never
    /// repaired from its own members.
    fn repair_nulls(&mut self) {
        let Some(nulls) = self.nulls.as_ref() else {
            return;
        };
        let cfa_type = CfaType::or_mono(self.metadata.cfa_type.as_ref());
        let neighbors = SameColorMedian::new(cfa_type);
        let size = Size2us::new(self.data.width(), self.data.height());
        for index in 0..size.pixel_count() {
            if nulls.is_null(index) {
                self.data[index] =
                    neighbors.at(&self.data, size.point_of(index), Some(nulls.bits()));
            }
        }
    }

    /// Demosaic this CFA image into a 3-channel LinearImage.
    /// Consumes self.
    pub(crate) fn demosaic(mut self, cancel: &CancelToken) -> Result<LinearImage, DemosaicError> {
        self.repair_nulls();
        let width = self.data.width();
        let height = self.data.height();
        let mut metadata = self.metadata;
        let cfa_type = metadata
            .cfa_type
            .clone()
            .expect("CfaImage missing cfa_type: set metadata.cfa_type before calling demosaic()");
        if let Some(provenance) = &mut metadata.provenance {
            let (color, demosaic) = match &cfa_type {
                CfaType::Mono => (ColorProvenance::Monochrome, DemosaicProvenance::None),
                CfaType::Bayer(_) => (ColorProvenance::SensorRgb, DemosaicProvenance::LumosRcd),
                CfaType::XTrans(_) => (
                    ColorProvenance::SensorRgb,
                    DemosaicProvenance::LumosMarkesteijn,
                ),
            };
            provenance.color = color;
            provenance.demosaic = demosaic;
        }
        let pixels = self.data.into_vec();
        // The mask travels at its own extent, which `repair_nulls` above is what makes honest: these
        // pixels were reconstructed rather than measured, and the combine still has to know that.
        let nulls = self.nulls;

        Ok(match &cfa_type {
            CfaType::Mono => {
                // No demosaicing needed - convert 1-channel to 1-channel LinearImage
                let dims = ImageDimensions::new((width, height), 1);
                let mut image = LinearImage::from_pixels(dims, pixels);
                image.metadata = metadata;
                image.nulls = nulls;
                image
            }
            CfaType::Bayer(cfa_pattern) => {
                use crate::io::raw::demosaic::bayer::{BayerImage, rcd};

                // Already cropped to the visible area: raw and active extents coincide and
                // there is no margin to skip.
                let size = Size2us::new(width, height);
                let bayer =
                    BayerImage::with_margins(&pixels, size, size, Vec2us::ZERO, *cfa_pattern);
                let planes = rcd::demosaic(&bayer, cancel)?;
                let dims = ImageDimensions::new((width, height), 3);
                let mut image = LinearImage::from_planar_channels(dims, planes);
                image.metadata = metadata;
                image.nulls = nulls;
                image
            }
            CfaType::XTrans(pattern) => {
                use crate::io::raw::demosaic::xtrans::process_xtrans_f32;

                // Already cropped to the visible area: raw and active extents coincide and
                // there is no margin to skip.
                let size = Size2us::new(width, height);
                let planes =
                    process_xtrans_f32(&pixels, size, size, Vec2us::ZERO, *pattern, cancel)?;

                let dims = ImageDimensions::new((width, height), 3);
                let mut image = LinearImage::from_planar_channels(dims, planes);
                image.metadata = metadata;
                image.nulls = nulls;
                image
            }
        })
    }

    /// Subtract another CfaImage pixel-by-pixel (dark subtraction).
    ///
    /// May produce negative pixel values when dark noise exceeds signal.
    /// This is intentional: the f32 pipeline preserves negatives, and stacking
    /// averages them out correctly. Clamping to zero would introduce a positive
    /// bias in the stacked result.
    pub fn subtract(&mut self, dark: &CfaImage) {
        assert!(
            self.data.width() == dark.data.width() && self.data.height() == dark.data.height(),
            "CfaImage dimensions mismatch: {}x{} vs {}x{}",
            self.data.width(),
            self.data.height(),
            dark.data.width(),
            dark.data.height()
        );
        // An invariant the caller upholds, not bad input: `CalibrationMasters` rejects a
        // domain-mismatched master before reaching here, and every other caller pairs frames it
        // decoded itself. Only checked when both declare a domain — a synthesized frame has none.
        debug_assert!(
            match (self.metadata.sample_domain(), dark.metadata.sample_domain()) {
                (Some(light), Some(dark)) => light.commensurate_with(&dark),
                _ => true,
            },
            "CfaImage sample domain mismatch: {:?} vs {:?}",
            self.metadata.sample_domain(),
            dark.metadata.sample_domain()
        );
        self.data
            .par_iter_mut()
            .zip(dark.data.par_iter())
            .for_each(|(l, d)| *l -= d);
    }
}

#[cfg(test)]
mod tests;
