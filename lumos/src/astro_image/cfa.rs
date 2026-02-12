//! Raw CFA (Color Filter Array) image representation.
//!
//! Represents sensor data before demosaicing - a single channel with color
//! filter pattern metadata. Used for calibration frame processing (darks,
//! flats, bias) and hot pixel correction on raw data.

use rayon::prelude::*;

use super::{AstroImage, AstroImageMetadata, ImageDimensions, PixelData};
use crate::common::Buffer2;
use crate::raw::demosaic::CfaPattern;

/// CFA pattern for raw sensor data.
#[derive(Debug, Clone)]
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
}

/// Raw CFA image - single channel with color filter pattern metadata.
/// Represents sensor data before demosaicing.
#[derive(Debug, Clone)]
pub struct CfaImage {
    /// Single-channel pixel data, normalized to 0.0-1.0.
    /// Layout: row-major, width * height pixels.
    pub data: Buffer2<f32>,
    pub metadata: AstroImageMetadata,
}

impl crate::stacking::cache::StackableImage for CfaImage {
    fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new(self.data.width(), self.data.height(), 1)
    }

    fn channel(&self, c: usize) -> &[f32] {
        assert!(c == 0, "CfaImage has only 1 channel, got {c}");
        &self.data
    }

    fn metadata(&self) -> &AstroImageMetadata {
        &self.metadata
    }

    fn load(path: &std::path::Path) -> Result<Self, crate::stacking::Error> {
        crate::raw::load_raw_cfa(path).map_err(|e| crate::stacking::Error::ImageLoad {
            path: path.to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })
    }

    fn from_stacked(
        pixels: PixelData,
        metadata: AstroImageMetadata,
        _dimensions: ImageDimensions,
    ) -> Self {
        CfaImage {
            data: pixels.into_l(),
            metadata,
        }
    }
}

impl CfaImage {
    /// Demosaic this CFA image into a 3-channel AstroImage.
    /// Consumes self.
    pub fn demosaic(self) -> AstroImage {
        let width = self.data.width();
        let height = self.data.height();
        let cfa_type = self.metadata.cfa_type.clone().unwrap();
        let pixels = self.data.into_vec();

        match &cfa_type {
            CfaType::Mono => {
                // No demosaicing needed - convert 1-channel to 1-channel AstroImage
                let dims = ImageDimensions::new(width, height, 1);
                let mut astro = AstroImage::from_pixels(dims, pixels);
                astro.metadata = self.metadata;
                astro
            }
            CfaType::Bayer(cfa_pattern) => {
                use crate::raw::demosaic::bayer::{BayerImage, demosaic_bayer};

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
                let rgb = demosaic_bayer(&bayer);
                let dims = ImageDimensions::new(width, height, 3);
                let mut astro = AstroImage::from_pixels(dims, rgb);
                astro.metadata = self.metadata;
                astro
            }
            CfaType::XTrans(pattern) => {
                use crate::raw::demosaic::xtrans::process_xtrans;

                // Convert f32 back to u16 for the existing XTrans pipeline.
                // The xtrans pipeline expects u16 + normalization params.
                let raw_u16: Vec<u16> = pixels
                    .par_iter()
                    .map(|&v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
                    .collect();

                let rgb = process_xtrans(
                    &raw_u16,
                    width,
                    height,
                    width,
                    height,
                    0,
                    0,
                    *pattern,
                    0.0,           // black = 0 (already normalized)
                    1.0 / 65535.0, // inv_range to undo the u16 conversion
                );

                let dims = ImageDimensions::new(width, height, 3);
                let mut astro = AstroImage::from_pixels(dims, rgb);
                astro.metadata = self.metadata;
                astro
            }
        }
    }

    /// Subtract another CfaImage pixel-by-pixel (dark subtraction).
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

    /// Divide by normalized flat (with optional bias subtraction from flat).
    /// Formula: light /= (flat - bias) / mean(flat - bias)
    pub fn divide_by_normalized(&mut self, flat: &CfaImage, bias: Option<&CfaImage>) {
        assert!(
            self.data.width() == flat.data.width() && self.data.height() == flat.data.height(),
            "Flat dimensions mismatch: {}x{} vs {}x{}",
            self.data.width(),
            self.data.height(),
            flat.data.width(),
            flat.data.height()
        );

        let flat_mean = if let Some(bias) = bias {
            assert!(
                bias.data.width() == flat.data.width() && bias.data.height() == flat.data.height(),
                "Bias dimensions mismatch: {}x{} vs {}x{}",
                bias.data.width(),
                bias.data.height(),
                flat.data.width(),
                flat.data.height()
            );
            let sum: f64 = flat
                .data
                .par_iter()
                .zip(bias.data.par_iter())
                .map(|(f, b)| (f - b) as f64)
                .sum();
            (sum / flat.data.len() as f64) as f32
        } else {
            let sum: f64 = flat.data.par_iter().map(|&f| f as f64).sum();
            (sum / flat.data.len() as f64) as f32
        };

        assert!(
            flat_mean > f32::EPSILON,
            "Flat frame mean is zero or negative after bias subtraction"
        );
        let inv_mean = 1.0 / flat_mean;

        match bias {
            Some(bias) => {
                self.data
                    .par_iter_mut()
                    .zip(flat.data.par_iter().zip(bias.data.par_iter()))
                    .for_each(|(l, (f, b))| {
                        let norm_flat = (f - b) * inv_mean;
                        if norm_flat > f32::EPSILON {
                            *l /= norm_flat;
                        }
                    });
            }
            None => {
                self.data
                    .par_iter_mut()
                    .zip(flat.data.par_iter())
                    .for_each(|(l, f)| {
                        let norm_flat = f * inv_mean;
                        if norm_flat > f32::EPSILON {
                            *l /= norm_flat;
                        }
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cfa(width: usize, height: usize, pixels: Vec<f32>, cfa_type: CfaType) -> CfaImage {
        CfaImage {
            data: Buffer2::new(width, height, pixels),
            metadata: AstroImageMetadata {
                cfa_type: Some(cfa_type),
                ..Default::default()
            },
        }
    }

    // ====================================================================
    // CfaType tests
    // ====================================================================

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

    // ====================================================================
    // CfaImage calibration tests
    // ====================================================================

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
    fn test_divide_by_normalized_no_bias() {
        // Flat with uniform value 0.8 → normalized flat = 0.8/0.8 = 1.0
        // Light should be unchanged
        let mut light = make_cfa(2, 2, vec![0.5, 0.6, 0.7, 0.8], CfaType::Mono);
        let flat = make_cfa(2, 2, vec![0.8, 0.8, 0.8, 0.8], CfaType::Mono);

        light.divide_by_normalized(&flat, None);

        assert!((light.data[0] - 0.5).abs() < 1e-6);
        assert!((light.data[1] - 0.6).abs() < 1e-6);
        assert!((light.data[2] - 0.7).abs() < 1e-6);
        assert!((light.data[3] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_divide_by_normalized_with_vignetting() {
        // Flat simulating vignetting: center bright, edges dim
        // flat = [0.5, 1.0, 1.0, 0.5], mean = 0.75
        // normalized flat = [0.5/0.75, 1.0/0.75, 1.0/0.75, 0.5/0.75]
        //                 = [0.667, 1.333, 1.333, 0.667]
        // light / norm_flat corrects vignetting
        let mut light = make_cfa(2, 2, vec![0.25, 0.5, 0.5, 0.25], CfaType::Mono);
        let flat = make_cfa(2, 2, vec![0.5, 1.0, 1.0, 0.5], CfaType::Mono);

        light.divide_by_normalized(&flat, None);

        // 0.25 / (0.5/0.75) = 0.25 / 0.6667 = 0.375
        assert!((light.data[0] - 0.375).abs() < 1e-4);
        // 0.5 / (1.0/0.75) = 0.5 / 1.3333 = 0.375
        assert!((light.data[1] - 0.375).abs() < 1e-4);
    }

    #[test]
    fn test_divide_by_normalized_with_bias() {
        // flat = [0.6, 0.6, 0.6, 0.6]
        // bias = [0.1, 0.1, 0.1, 0.1]
        // flat - bias = [0.5, 0.5, 0.5, 0.5], mean = 0.5
        // normalized = 1.0 everywhere → light unchanged
        let mut light = make_cfa(2, 2, vec![0.4, 0.4, 0.4, 0.4], CfaType::Mono);
        let flat = make_cfa(2, 2, vec![0.6, 0.6, 0.6, 0.6], CfaType::Mono);
        let bias = make_cfa(2, 2, vec![0.1, 0.1, 0.1, 0.1], CfaType::Mono);

        light.divide_by_normalized(&flat, Some(&bias));

        for &v in light.data.pixels() {
            assert!((v - 0.4).abs() < 1e-6);
        }
    }

    #[test]
    fn test_data_len() {
        let img = make_cfa(10, 20, vec![0.0; 200], CfaType::Mono);
        assert_eq!(img.data.len(), 200);
    }
}
