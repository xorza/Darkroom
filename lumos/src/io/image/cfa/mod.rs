//! Raw CFA (Color Filter Array) image representation.
//!
//! Represents sensor data before demosaicing - a single channel with color
//! filter pattern metadata. Used for calibration frame processing (darks,
//! flats, bias) and hot pixel correction on raw data.

pub(crate) mod same_color;

use std::io;
use std::path::Path;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::same_color::SameColorMedian;
use crate::io::image::error::ImageError;
use crate::io::image::fits::{cfa as fits_cfa, decode as fits_decode};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{ColorProvenance, DemosaicProvenance};
use crate::io::image::input_format::InputFormat;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::io::image::standard::scientific_rejection;
use crate::io::raw;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::bayer::rcd;
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::io::raw::demosaic::xtrans::markesteijn;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::{XTransPattern, XTransPatternError};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::frame_store::{FramePeek, StackableImage};
use common::CancelToken;
use imaginarium::Buffer2;

/// Standard deviation of uniform error spanning one ADC step: `1 / √12`.
pub(crate) const QUANTIZATION_SIGMA_PER_STEP: f32 = 0.288_675_13;

/// The colour filter over a sensor's photosites, anchored at the origin of the image data it
/// accompanies. The one sensor-pattern type: a frame with no filter is [`CfaType::Mono`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfaType {
    /// No CFA pattern (monochrome sensor)
    Mono,
    /// 2x2 Bayer pattern
    Bayer(CfaPattern),
    /// 6x6 X-Trans pattern
    XTrans(XTransPattern),
}

impl CfaType {
    /// Get the color index (0=R, 1=G, 2=B) at position (x, y).
    /// For Mono, always returns 0.
    #[inline(always)]
    pub fn color_at(&self, pos: Vec2us) -> u8 {
        match self {
            CfaType::Mono => 0,
            CfaType::Bayer(p) => p.color_at(pos) as u8,
            CfaType::XTrans(pattern) => pattern.color_at(pos),
        }
    }

    /// Number of distinct color channels (1 for Mono, 3 for Bayer/X-Trans).
    pub fn num_colors(&self) -> usize {
        match self {
            CfaType::Mono => 1,
            CfaType::Bayer(_) | CfaType::XTrans(_) => 3,
        }
    }

    /// The mosaic a camera-RAW sensor delivers, from LibRaw's `filters` and `colors` and its
    /// visible-origin X-Trans pattern, or `None` when LibRaw has to produce the image itself.
    ///
    /// `None` covers `filters == 0` with three colours — a linear DNG, sRAW or Foveon, whose
    /// samples LibRaw unpacks already per pixel and so never as a mosaic — and a filter word that
    /// is neither X-Trans nor a 2×2 Bayer phase. An X-Trans sensor whose layout is not one is an
    /// error: the file is corrupt, not exotic.
    pub(crate) fn from_libraw(
        filters: u32,
        colors: i32,
        xtrans: [[u8; 6]; 6],
    ) -> Result<Option<Self>, XTransPatternError> {
        if colors == 1 {
            return Ok(Some(Self::Mono));
        }
        Ok(match filters {
            0 => None,
            // LibRaw's marker for the 6×6 X-Trans layout, whose pattern it keeps apart.
            9 => Some(Self::XTrans(XTransPattern::new(xtrans)?)),
            _ => CfaPattern::from_filters(filters).map(Self::Bayer),
        })
    }

    /// The memory a demosaic of a `dimensions` frame of this pattern holds at once.
    pub(crate) fn demosaic_memory(self, dimensions: ImageDimensions) -> DemosaicMemory {
        match self {
            Self::Mono => {
                let bytes = dimensions.pixel_count().saturating_mul(size_of::<f32>());
                DemosaicMemory {
                    output_bytes: bytes,
                    peak_bytes: bytes,
                }
            }
            // Raw and active extents coincide here: the caller has already cropped to the
            // visible area, so the margins the RCD arena would need are gone.
            Self::Bayer(_) => rcd::demosaic_memory(dimensions.size(), dimensions.size()),
            Self::XTrans(_) => markesteijn::demosaic_memory(dimensions.size()),
        }
    }

    /// The demosaic lumos runs for this pattern.
    pub(crate) const fn demosaic_provenance(self) -> DemosaicProvenance {
        match self {
            Self::Mono => DemosaicProvenance::None,
            Self::Bayer(_) => DemosaicProvenance::LumosRcd,
            Self::XTrans(_) => DemosaicProvenance::LumosMarkesteijn,
        }
    }

    /// What the colour of a demosaiced frame of this pattern means.
    pub(crate) const fn demosaiced_color(self) -> ColorProvenance {
        match self {
            Self::Mono => ColorProvenance::Monochrome,
            Self::Bayer(_) | Self::XTrans(_) => ColorProvenance::SensorRgb,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CfaFrameInfo {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) cfa_type: CfaType,
    /// Whether decoding could produce pixels with no measurement, which a frame pays two quality
    /// planes for beside its own. Answered from the header alone, so it is conservative where the
    /// header cannot settle it — see `FitsDecodePlan::may_carry_nulls`.
    pub(crate) may_carry_nulls: bool,
}

impl CfaFrameInfo {
    pub(crate) fn from_file(path: &Path, context: &LoadContext) -> Result<Self, ImageError> {
        match InputFormat::of(path)? {
            InputFormat::Fits => fits_decode::fits_cfa_frame_info(path, context),
            InputFormat::CameraRaw => raw::raw_cfa_frame_info(path, context),
            InputFormat::Raster(_) => Err(scientific_rejection(
                path,
                "scientific CFA input must be camera RAW or FITS",
            )),
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
    pub cfa_type: CfaType,
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

    fn cfa_type(&self) -> Option<CfaType> {
        Some(self.cfa_type)
    }

    fn quantization_sigma(&self) -> Option<f32> {
        self.quantization_sigma
    }

    fn load(path: &Path, context: &LoadContext) -> Result<Self, ImageError> {
        CfaImage::from_file(path, context)
    }

    fn peek(path: &Path, context: &LoadContext) -> Option<FramePeek> {
        CfaFrameInfo::from_file(path, context)
            .ok()
            .map(FramePeek::from)
    }

    fn into_planes(self) -> arrayvec::ArrayVec<Buffer2<f32>, 3> {
        let mut planes = arrayvec::ArrayVec::new();
        planes.push(self.data);
        planes
    }
}

impl CfaImage {
    /// Create an in-memory sensor image whose CFA classification is supplied by the caller.
    pub fn from_plane(data: Buffer2<f32>, cfa_type: CfaType, metadata: ImageMetadata) -> Self {
        Self {
            data,
            cfa_type,
            metadata,
            quantization_sigma: None,
            nulls: None,
        }
    }

    /// Load an un-demosaiced sensor image from camera RAW or mosaic FITS.
    pub fn from_file<P: AsRef<Path>>(path: P, context: &LoadContext) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => fits_decode::load_cfa_fits(path, context),
            InputFormat::CameraRaw => raw::load_raw_cfa(path, context),
            InputFormat::Raster(_) => Err(scientific_rejection(
                path,
                "generic raster decoders do not establish a scientific CFA contract",
            )),
        }
    }

    /// Resident RAM held by this frame: its single f32 CFA plane's pixel bytes.
    /// Metadata is negligible against a full-sensor plane.
    pub fn ram_bytes(&self) -> usize {
        self.data.width() * self.data.height() * size_of::<f32>()
    }

    /// Save this sensor-domain image as a checksummed floating-point FITS file.
    pub fn save_fits(&self, path: &Path) -> io::Result<()> {
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
        let neighbors = SameColorMedian::new(&self.cfa_type);
        let size = Size2us::new(self.data.width(), self.data.height());
        for index in 0..size.pixel_count() {
            if nulls.is_null(index) {
                self.data[index] =
                    neighbors.at(&self.data, size.point_of(index), Some(nulls.bits()));
            }
        }
    }

    /// Demosaic this CFA image into a 3-channel `LinearImage`.
    /// Consumes self.
    pub(crate) fn demosaic(mut self, cancel: &CancelToken) -> Result<LinearImage, Cancelled> {
        self.repair_nulls();
        let width = self.data.width();
        let height = self.data.height();
        let mut metadata = self.metadata;
        let cfa_type = self.cfa_type;
        if let Some(provenance) = &mut metadata.provenance {
            provenance.color = cfa_type.demosaiced_color();
            provenance.demosaic = cfa_type.demosaic_provenance();
        }
        let pixels = self.data.into_vec();
        // The mask travels at its own extent, which `repair_nulls` above is what makes honest: these
        // pixels were reconstructed rather than measured, and the combine still has to know that.
        let nulls = self.nulls;

        Ok(match cfa_type {
            CfaType::Mono => {
                // No demosaicing needed - convert 1-channel to 1-channel LinearImage
                let dims = ImageDimensions::new((width, height), 1);
                let mut image = LinearImage::from_pixels(dims, pixels);
                image.metadata = metadata;
                image.nulls = nulls;
                image
            }
            CfaType::Bayer(cfa_pattern) => {
                use crate::io::raw::demosaic::bayer::BayerImage;

                let layout = SensorLayout::cropped(Size2us::new(width, height));
                let bayer = BayerImage::with_margins(&pixels, layout, cfa_pattern);
                let planes = rcd::demosaic(&bayer, cancel)?;
                let dims = ImageDimensions::new((width, height), 3);
                let mut image = LinearImage::from_planar_channels(dims, planes);
                image.metadata = metadata;
                image.nulls = nulls;
                image
            }
            CfaType::XTrans(pattern) => {
                use crate::io::raw::demosaic::xtrans::process_xtrans_f32;

                let layout = SensorLayout::cropped(Size2us::new(width, height));
                let planes = process_xtrans_f32(&pixels, layout, pattern, cancel)?;

                let dims = ImageDimensions::new((width, height), 3);
                let mut image = LinearImage::from_planar_channels(dims, planes);
                image.metadata = metadata;
                image.nulls = nulls;
                image
            }
        })
    }

    /// Subtract another `CfaImage` pixel-by-pixel (dark subtraction), each of its samples first
    /// multiplied by `dark_scale` — the factor that expresses it in this frame's domain
    /// ([`SampleDomain::conversion_to`](crate::SampleDomain::conversion_to)). A factor of exactly
    /// one subtracts the samples as they are.
    ///
    /// May produce negative pixel values when dark noise exceeds signal.
    /// This is intentional: the f32 pipeline preserves negatives, and stacking
    /// averages them out correctly. Clamping to zero would introduce a positive
    /// bias in the stacked result.
    pub fn subtract(&mut self, dark: &CfaImage, dark_scale: f32) {
        assert!(
            self.data.width() == dark.data.width() && self.data.height() == dark.data.height(),
            "CfaImage dimensions mismatch: {}x{} vs {}x{}",
            self.data.width(),
            self.data.height(),
            dark.data.width(),
            dark.data.height()
        );
        // An invariant the caller upholds, not bad input: `CalibrationMasters` derives the factor
        // from the two domains and refuses a pair with none. Only checked when both declare a
        // domain — a synthesized frame has none.
        debug_assert!(
            match (self.metadata.sample_domain(), dark.metadata.sample_domain()) {
                (Some(light), Some(dark)) => dark.conversion_to(&light) == Some(dark_scale),
                _ => true,
            },
            "dark_scale {dark_scale} does not convert {:?} into {:?}",
            dark.metadata.sample_domain(),
            self.metadata.sample_domain()
        );
        if dark_scale == 1.0 {
            self.data
                .par_iter_mut()
                .zip(dark.data.par_iter())
                .for_each(|(l, d)| *l -= d);
        } else {
            self.data
                .par_iter_mut()
                .zip(dark.data.par_iter())
                .for_each(|(l, d)| *l -= d * dark_scale);
        }
    }
}

#[cfg(test)]
mod tests;
