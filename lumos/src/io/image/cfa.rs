//! Raw CFA (Color Filter Array) image representation.
//!
//! Represents sensor data before demosaicing - a single channel with color
//! filter pattern metadata. Used for calibration frame processing (darks,
//! flats, bias) and hot pixel correction on raw data.

use std::path::Path;

use rayon::prelude::*;

use crate::io::image::error::ImageError;
use crate::io::image::fits;
use crate::io::image::linear::LinearImage;
use crate::io::image::load::LoadContext;
use crate::io::image::{
    ColorProvenance, DemosaicProvenance, FITS_EXTENSIONS, ImageDimensions, ImageMetadata,
    STANDARD_IMAGE_EXTENSIONS, file_extension, scientific_rejection,
};
use crate::io::raw;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::{DemosaicError, DemosaicKind};
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
    pub fn color_at(&self, x: usize, y: usize) -> u8 {
        match self {
            CfaType::Mono => 0,
            CfaType::Bayer(p) => p.color_at(y, x) as u8,
            CfaType::XTrans(pattern) => pattern[y % 6][x % 6],
        }
    }

    /// Number of distinct color channels (1 for Mono, 3 for Bayer/X-Trans).
    pub fn num_colors(&self) -> usize {
        match self {
            CfaType::Mono => 1,
            CfaType::Bayer(_) | CfaType::XTrans(_) => 3,
        }
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
            fits::fits_cfa_frame_info(path, context)
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
}

impl StackableImage for CfaImage {
    fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new((self.data.width(), self.data.height()), 1)
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
        }
    }

    /// Load an un-demosaiced sensor image from camera RAW or mosaic FITS.
    pub fn from_file<P: AsRef<Path>>(
        path: P,
        context: &LoadContext,
    ) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        let extension = file_extension(path);

        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            return fits::load_cfa_fits(path, context);
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
        fits::save_cfa_fits(path, self)
    }

    /// Demosaic this CFA image into a 3-channel LinearImage.
    /// Consumes self.
    pub(crate) fn demosaic(self, cancel: &CancelToken) -> Result<LinearImage, DemosaicError> {
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

        Ok(match &cfa_type {
            CfaType::Mono => {
                // No demosaicing needed - convert 1-channel to 1-channel LinearImage
                let dims = ImageDimensions::new((width, height), 1);
                let mut image = LinearImage::from_pixels(dims, pixels);
                image.metadata = metadata;
                image
            }
            CfaType::Bayer(cfa_pattern) => {
                use crate::io::raw::demosaic::bayer::{BayerImage, rcd};

                let bayer = BayerImage::with_margins(
                    &pixels,
                    width,
                    height,
                    width,
                    height,
                    0,
                    0,
                    *cfa_pattern,
                );
                let planes = rcd::demosaic(&bayer, cancel)?;
                let dims = ImageDimensions::new((width, height), 3);
                let mut image = LinearImage::from_planar_channels(dims, planes);
                image.metadata = metadata;
                image
            }
            CfaType::XTrans(pattern) => {
                use crate::io::raw::demosaic::xtrans::process_xtrans_f32;

                let planes = process_xtrans_f32(
                    &pixels, width, height, width, height, 0, 0, *pattern, cancel,
                )?;

                let dims = ImageDimensions::new((width, height), 3);
                let mut image = LinearImage::from_planar_channels(dims, planes);
                image.metadata = metadata;
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
        self.data
            .par_iter_mut()
            .zip(dark.data.par_iter())
            .for_each(|(l, d)| *l -= d);
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::cfa::*;
    use crate::io::image::load::LoadContext;
    use crate::io::image::error::ImageError;
    use crate::io::raw::demosaic::DemosaicKind;
    use crate::io::raw::demosaic::xtrans::test_support::test_pattern_array;
    use crate::testing::make_cfa;

    #[test]
    fn master_cfa_save_load_round_trips_data_and_pattern() {
        let cfa = CfaImage {
            data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
            metadata: ImageMetadata {
                cfa_type: Some(CfaType::Bayer(CfaPattern::Bggr)),
                camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
                ..Default::default()
            },
            quantization_sigma: Some(0.000_01),
        };
        let path = common::test_utils::test_output_path("cfa_master_roundtrip.fits");
        cfa.save_fits(&path).unwrap();
        let info = CfaFrameInfo::from_file(&path, &LoadContext::default()).unwrap();
        assert_eq!(info.dimensions, ImageDimensions::new((2, 2), 1));
        assert_eq!(info.demosaic, DemosaicKind::BayerRcd);
        let loaded = CfaImage::from_file(&path, &LoadContext::default()).unwrap();

        assert_eq!((loaded.data.width(), loaded.data.height()), (2, 2));
        assert_eq!(loaded.data.to_vec(), vec![0.1f32, 0.2, 0.3, 0.4]);
        assert!(matches!(
            loaded.metadata.cfa_type,
            Some(CfaType::Bayer(CfaPattern::Bggr))
        ));
        assert_eq!(
            loaded.metadata.camera_white_balance,
            Some([2.0, 1.0, 1.5, 1.0])
        );
        assert_eq!(loaded.quantization_sigma, Some(0.000_01));

        let original = std::fs::read(&path).unwrap();
        let mut invalid_version = original.clone();
        let version_card = invalid_version
            .windows(8)
            .position(|window| window == b"LUMOSVER")
            .unwrap();
        let version_digit = invalid_version[version_card..version_card + 80]
            .iter()
            .rposition(u8::is_ascii_digit)
            .unwrap();
        invalid_version[version_card + version_digit] = b'0';
        std::fs::write(&path, invalid_version).unwrap();
        assert!(matches!(
            CfaImage::from_file(&path, &LoadContext::default()),
            Err(ImageError::FitsUnsupported { reason, .. }) if reason.contains("version")
        ));

        let mut corrupted = original.clone();
        let sample = 0.1f32.to_be_bytes();
        let offset = corrupted
            .windows(sample.len())
            .position(|window| window == sample)
            .unwrap();
        corrupted[offset] ^= 0x01;
        std::fs::write(&path, corrupted).unwrap();
        assert!(matches!(
            CfaImage::from_file(&path, &LoadContext::default()),
            Err(ImageError::FitsUnsupported { reason, .. }) if reason.contains("checksum")
        ));
        std::fs::write(path, original).unwrap();
    }

    #[test]
    fn master_cfa_fits_round_trips_mono_and_xtrans_patterns() {
        for (name, cfa_type) in [
            ("mono", CfaType::Mono),
            ("xtrans", CfaType::XTrans(test_pattern_array())),
        ] {
            let image = CfaImage {
                data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
                metadata: ImageMetadata {
                    cfa_type: Some(cfa_type.clone()),
                    ..Default::default()
                },
                quantization_sigma: None,
            };
            let path = common::test_utils::test_output_path(&format!("cfa_master_{name}.fits"));

            image.save_fits(&path).unwrap();
            let loaded = CfaImage::from_file(path, &LoadContext::default()).unwrap();

            assert_eq!(loaded.metadata.cfa_type, Some(cfa_type), "{name}");
            assert_eq!(loaded.data.pixels(), image.data.pixels(), "{name}");
        }
    }

    #[test]
    fn test_cfa_type_mono_color_at() {
        let mono = CfaType::Mono;
        assert_eq!(mono.color_at(0, 0), 0);
        assert_eq!(mono.color_at(5, 5), 0);
    }

    #[test]
    fn test_cfa_type_bayer_rggb_color_at() {
        let bayer = CfaType::Bayer(CfaPattern::Rggb);
        // RGGB: (x=0,y=0)=R, (x=1,y=0)=G, (x=0,y=1)=G, (x=1,y=1)=B
        assert_eq!(bayer.color_at(0, 0), 0); // R
        assert_eq!(bayer.color_at(1, 0), 1); // G
        assert_eq!(bayer.color_at(0, 1), 1); // G
        assert_eq!(bayer.color_at(1, 1), 2); // B
    }

    #[test]
    fn test_cfa_type_bayer_bggr_color_at() {
        let bayer = CfaType::Bayer(CfaPattern::Bggr);
        // BGGR: (x=0,y=0)=B, (x=1,y=0)=G, (x=0,y=1)=G, (x=1,y=1)=R
        assert_eq!(bayer.color_at(0, 0), 2); // B
        assert_eq!(bayer.color_at(1, 0), 1); // G
        assert_eq!(bayer.color_at(0, 1), 1); // G
        assert_eq!(bayer.color_at(1, 1), 0); // R
    }

    #[test]
    fn test_cfa_type_bayer_wrapping() {
        let bayer = CfaType::Bayer(CfaPattern::Rggb);
        // Pattern repeats every 2 pixels
        assert_eq!(bayer.color_at(0, 0), bayer.color_at(2, 0));
        assert_eq!(bayer.color_at(0, 0), bayer.color_at(0, 2));
        assert_eq!(bayer.color_at(1, 1), bayer.color_at(3, 3));
    }

    #[test]
    fn test_cfa_type_xtrans_color_at() {
        let pattern = [
            [1, 0, 1, 1, 2, 1],
            [2, 1, 2, 0, 1, 0],
            [1, 2, 1, 1, 0, 1],
            [1, 2, 1, 1, 0, 1],
            [0, 1, 0, 2, 1, 2],
            [1, 0, 1, 1, 2, 1],
        ];
        let xtrans = CfaType::XTrans(pattern);
        assert_eq!(xtrans.color_at(0, 0), 1); // G
        assert_eq!(xtrans.color_at(1, 0), 0); // R
        assert_eq!(xtrans.color_at(0, 1), 2); // B
        // Wrapping
        assert_eq!(xtrans.color_at(6, 0), xtrans.color_at(0, 0));
        assert_eq!(xtrans.color_at(0, 6), xtrans.color_at(0, 0));
    }

    #[test]
    fn test_subtract() {
        let mut light = make_cfa(2, 2, vec![0.5, 0.6, 0.7, 0.8], CfaType::Mono);
        let dark = make_cfa(2, 2, vec![0.1, 0.1, 0.1, 0.1], CfaType::Mono);

        light.subtract(&dark);

        assert!((light.data[0] - 0.4).abs() < 1e-6);
        assert!((light.data[1] - 0.5).abs() < 1e-6);
        assert!((light.data[2] - 0.6).abs() < 1e-6);
        assert!((light.data[3] - 0.7).abs() < 1e-6);
    }

    #[test]
    #[should_panic(expected = "dimensions mismatch")]
    fn test_subtract_dimension_mismatch() {
        let mut light = make_cfa(2, 2, vec![0.5; 4], CfaType::Mono);
        let dark = make_cfa(3, 3, vec![0.1; 9], CfaType::Mono);
        light.subtract(&dark);
    }

    #[test]
    fn test_data_len() {
        let img = CfaImage::from_plane(
            Buffer2::new(10, 20, vec![0.0; 200]),
            ImageMetadata {
                cfa_type: Some(CfaType::Mono),
                ..ImageMetadata::default()
            },
        );
        assert_eq!(img.data.len(), 200);
        assert_eq!(img.quantization_sigma, None);
    }
}
