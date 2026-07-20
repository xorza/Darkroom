//! Raw CFA (Color Filter Array) image representation.
//!
//! Represents sensor data before demosaicing - a single channel with color
//! filter pattern metadata. Used for calibration frame processing (darks,
//! flats, bias) and hot pixel correction on raw data.

use std::io::Error;

use rayon::prelude::*;

use common::file_utils;

use crate::io::astro_image::error::ImageError;
use crate::io::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
use crate::io::raw::demosaic::Cancelled;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::{load_raw_cfa, raw_dimensions};
use crate::stacking::frame_store::StackableImage;
use common::CancelToken;
use imaginarium::Buffer2;

/// Standard deviation of uniform error spanning one ADC step: `1 / √12`.
pub(crate) const QUANTIZATION_SIGMA_PER_STEP: f32 = 0.288_675_13;

/// CFA pattern anchored at the origin of the image data it accompanies.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
}

/// Raw CFA image - single channel with color filter pattern metadata.
/// Represents sensor data before demosaicing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CfaImage {
    /// Single-channel linear samples; calibration may put values outside `[0, 1]`.
    /// Layout: row-major, width * height pixels.
    pub data: Buffer2<f32>,
    pub metadata: AstroImageMetadata,
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

    fn metadata(&self) -> &AstroImageMetadata {
        &self.metadata
    }

    fn quantization_sigma(&self) -> Option<f32> {
        self.quantization_sigma
    }

    fn load(path: &std::path::Path) -> Result<Self, ImageError> {
        load_raw_cfa(path)
    }

    /// CFA frames are always RAW, so the dimensions come straight from the RAW header — no decode.
    fn peek_dimensions(path: &std::path::Path) -> Option<ImageDimensions> {
        raw_dimensions(path)
            .ok()
            .map(|size| ImageDimensions::new(size, 1))
    }

    fn into_planes(self) -> arrayvec::ArrayVec<imaginarium::Buffer2<f32>, 3> {
        let mut planes = arrayvec::ArrayVec::new();
        planes.push(self.data);
        planes
    }
}

impl CfaImage {
    /// Resident RAM held by this frame: its single f32 CFA plane's pixel bytes.
    /// Metadata is negligible against a full-sensor plane.
    pub fn ram_bytes(&self) -> usize {
        self.data.width() * self.data.height() * std::mem::size_of::<f32>()
    }

    /// Serialize this CFA image to `path` (bitcode: the CFA plane + metadata).
    /// Pairs with [`Self::load`] for a calibration-master cache.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let bytes = common::serialize(self, common::SerdeFormat::Bitcode).map_err(Error::other)?;
        file_utils::publish_bytes(path, &bytes, file_utils::PublicationMode::Cache)
    }

    /// Load a CFA image written by [`Self::save`].
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        common::deserialize(&bytes, common::SerdeFormat::Bitcode).map_err(Error::other)
    }

    /// Demosaic this CFA image into a 3-channel AstroImage.
    /// Consumes self.
    pub(crate) fn demosaic(self, cancel: &CancelToken) -> Result<AstroImage, Cancelled> {
        let width = self.data.width();
        let height = self.data.height();
        let cfa_type =
            self.metadata.cfa_type.clone().expect(
                "CfaImage missing cfa_type: set metadata.cfa_type before calling demosaic()",
            );
        let pixels = self.data.into_vec();

        Ok(match &cfa_type {
            CfaType::Mono => {
                // No demosaicing needed - convert 1-channel to 1-channel AstroImage
                let dims = ImageDimensions::new((width, height), 1);
                let mut astro = AstroImage::from_pixels(dims, pixels);
                astro.metadata = self.metadata;
                astro
            }
            CfaType::Bayer(cfa_pattern) => {
                use crate::io::raw::demosaic::bayer::{BayerImage, demosaic_bayer};

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
                let planes = demosaic_bayer(&bayer, cancel)?;
                let dims = ImageDimensions::new((width, height), 3);
                let mut astro = AstroImage::from_planar_channels(dims, planes);
                astro.metadata = self.metadata;
                astro
            }
            CfaType::XTrans(pattern) => {
                use crate::io::raw::demosaic::xtrans::process_xtrans_f32;

                let planes = process_xtrans_f32(
                    &pixels, width, height, width, height, 0, 0, *pattern, cancel,
                )?;

                let dims = ImageDimensions::new((width, height), 3);
                let mut astro = AstroImage::from_planar_channels(dims, planes);
                astro.metadata = self.metadata;
                astro
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
    use crate::io::astro_image::cfa::*;
    use crate::testing::make_cfa;

    #[test]
    fn master_cfa_save_load_round_trips_data_and_pattern() {
        let cfa = CfaImage {
            data: Buffer2::new(2, 2, vec![0.1f32, 0.2, 0.3, 0.4]),
            metadata: AstroImageMetadata {
                cfa_type: Some(CfaType::Bayer(CfaPattern::Bggr)),
                camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
                ..Default::default()
            },
            quantization_sigma: Some(0.000_01),
        };
        let path = common::test_utils::test_output_path("cfa_master_roundtrip.lcm");
        cfa.save(&path).unwrap();
        let loaded = CfaImage::load(&path).unwrap();

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
        let img = make_cfa(10, 20, vec![0.0; 200], CfaType::Mono);
        assert_eq!(img.data.len(), 200);
    }
}
