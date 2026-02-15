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
            source: std::io::Error::other(e),
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
        let cfa_type =
            self.metadata.cfa_type.clone().expect(
                "CfaImage missing cfa_type: set metadata.cfa_type before calling demosaic()",
            );
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
                use crate::raw::demosaic::xtrans::process_xtrans_f32;

                let rgb = process_xtrans_f32(&pixels, width, height, width, height, 0, 0, *pattern);

                let dims = ImageDimensions::new(width, height, 3);
                let mut astro = AstroImage::from_pixels(dims, rgb);
                astro.metadata = self.metadata;
                astro
            }
        }
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

    /// Divide by normalized flat (with optional bias subtraction from flat).
    ///
    /// For Mono or missing CFA: uses single global mean.
    /// Formula: light /= (flat - bias) / mean(flat - bias)
    ///
    /// For Bayer/X-Trans: uses per-color-channel means to avoid color shift
    /// when flat light source is not perfectly white (LED panels, twilight flats).
    /// Each R/G/B channel is normalized independently.
    pub fn divide_by_normalized(&mut self, flat: &CfaImage, bias: Option<&CfaImage>) {
        assert!(
            self.data.width() == flat.data.width() && self.data.height() == flat.data.height(),
            "Flat dimensions mismatch: {}x{} vs {}x{}",
            self.data.width(),
            self.data.height(),
            flat.data.width(),
            flat.data.height()
        );

        if let Some(bias) = bias {
            assert!(
                bias.data.width() == flat.data.width() && bias.data.height() == flat.data.height(),
                "Bias dimensions mismatch: {}x{} vs {}x{}",
                bias.data.width(),
                bias.data.height(),
                flat.data.width(),
                flat.data.height()
            );
        }

        let cfa_type = flat
            .metadata
            .cfa_type
            .clone()
            .or_else(|| self.metadata.cfa_type.clone());
        let num_colors = cfa_type.as_ref().map_or(1, |c| c.num_colors());

        if num_colors == 1 {
            self.divide_by_normalized_mono(flat, bias);
        } else {
            self.divide_by_normalized_cfa(flat, bias, &cfa_type.unwrap());
        }
    }

    /// Single-mean flat normalization (Mono or unknown CFA).
    fn divide_by_normalized_mono(&mut self, flat: &CfaImage, bias: Option<&CfaImage>) {
        let flat_mean = if let Some(bias) = bias {
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

    /// Per-CFA-channel flat normalization (Bayer/X-Trans).
    /// Computes independent R/G/B means to avoid color shift from non-white flats.
    fn divide_by_normalized_cfa(
        &mut self,
        flat: &CfaImage,
        bias: Option<&CfaImage>,
        cfa_type: &CfaType,
    ) {
        let width = self.data.width();
        let height = self.data.height();

        // Accumulate per-color sums and counts
        let mut sums = [0.0f64; 3];
        let mut counts = [0u64; 3];

        for y in 0..height {
            let flat_row = flat.data.row(y);
            let bias_row = bias.map(|b| b.data.row(y));
            for x in 0..width {
                let color = cfa_type.color_at(x, y) as usize;
                let val = match bias_row {
                    Some(br) => (flat_row[x] - br[x]) as f64,
                    None => flat_row[x] as f64,
                };
                sums[color] += val;
                counts[color] += 1;
            }
        }

        let mut inv_means = [0.0f32; 3];
        for c in 0..3 {
            assert!(counts[c] > 0, "Flat has no pixels for color channel {c}");
            let mean = (sums[c] / counts[c] as f64) as f32;
            assert!(
                mean > f32::EPSILON,
                "Flat channel {c} mean is zero or negative"
            );
            inv_means[c] = 1.0 / mean;
        }

        // Apply per-channel normalization (row-parallel)
        let flat_data = &flat.data;
        let bias_data = bias.map(|b| &b.data);

        self.data
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                let flat_row = flat_data.row(y);
                let bias_row = bias_data.map(|b| b.row(y));
                for (x, pixel) in row.iter_mut().enumerate() {
                    let color = cfa_type.color_at(x, y) as usize;
                    let norm_flat = match bias_row {
                        Some(br) => (flat_row[x] - br[x]) * inv_means[color],
                        None => flat_row[x] * inv_means[color],
                    };
                    if norm_flat > f32::EPSILON {
                        *pixel /= norm_flat;
                    }
                }
            });
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

    // ====================================================================
    // Per-CFA-channel flat normalization tests
    // ====================================================================

    #[test]
    fn test_bayer_flat_non_white_light() {
        // 4×4 RGGB flat where R/G/B have different response levels,
        // simulating a non-white flat light source (e.g. LED panel).
        //
        // RGGB pattern for 4×4:
        //   (0,0)=R (1,0)=G (2,0)=R (3,0)=G
        //   (0,1)=G (1,1)=B (2,1)=G (3,1)=B
        //   (0,2)=R (1,2)=G (2,2)=R (3,2)=G
        //   (0,3)=G (1,3)=B (2,3)=G (3,3)=B
        //
        // Flat values: R=0.9, G=0.6, B=0.3
        // Per-channel means: mean_R=0.9, mean_G=0.6, mean_B=0.3
        // Per-channel inv_means: 1/0.9, 1/0.6, 1/0.3
        //
        // Normalized flat for each pixel = flat_val * inv_mean_color = 1.0
        // So uniform light divided by this flat should be unchanged.
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        let mut flat_pixels = vec![0.0f32; 16];
        for y in 0..4 {
            for x in 0..4 {
                let color = cfa.color_at(x, y);
                flat_pixels[y * 4 + x] = match color {
                    0 => 0.9, // R
                    1 => 0.6, // G
                    2 => 0.3, // B
                    _ => unreachable!(),
                };
            }
        }

        // Uniform light = 0.5 everywhere
        let mut light = make_cfa(4, 4, vec![0.5; 16], cfa.clone());
        let flat = make_cfa(4, 4, flat_pixels, cfa);

        light.divide_by_normalized(&flat, None);

        // Each pixel's norm_flat = flat_val / mean_of_its_color = 1.0
        // So light should be unchanged: 0.5 / 1.0 = 0.5
        for &v in light.data.pixels() {
            assert!((v - 0.5).abs() < 1e-5, "Expected 0.5, got {v}");
        }
    }

    #[test]
    fn test_bayer_flat_vignetting_with_color() {
        // 4×4 RGGB flat with both vignetting and color difference.
        // Center pixels brighter than edge pixels, R stronger than B.
        //
        // RGGB layout (same as above).
        // Use spatially varying flat to test both corrections.
        let cfa = CfaType::Bayer(CfaPattern::Rggb);

        // Flat: R channel values = [0.4, 0.8, 0.4, 0.8] (vignetting)
        //       G channel values all = 0.6
        //       B channel values all = 0.3
        // Actually, let's build a concrete 4×4 flat:
        //
        //   Row 0: R=0.4  G=0.6  R=0.8  G=0.6
        //   Row 1: G=0.6  B=0.3  G=0.6  B=0.3
        //   Row 2: R=0.8  G=0.6  R=0.4  G=0.6
        //   Row 3: G=0.6  B=0.3  G=0.6  B=0.3
        //
        // R pixels: 0.4, 0.8, 0.8, 0.4 → mean_R = 2.4/4 = 0.6
        // G pixels: 0.6, 0.6, 0.6, 0.6, 0.6, 0.6, 0.6, 0.6 → mean_G = 4.8/8 = 0.6
        // B pixels: 0.3, 0.3, 0.3, 0.3 → mean_B = 1.2/4 = 0.3
        let flat_pixels = vec![
            0.4, 0.6, 0.8, 0.6, // row 0
            0.6, 0.3, 0.6, 0.3, // row 1
            0.8, 0.6, 0.4, 0.6, // row 2
            0.6, 0.3, 0.6, 0.3, // row 3
        ];

        // Light: uniform 0.5
        let mut light = make_cfa(4, 4, vec![0.5; 16], cfa.clone());
        let flat = make_cfa(4, 4, flat_pixels, cfa.clone());

        light.divide_by_normalized(&flat, None);

        // For R pixel at (0,0): flat=0.4, inv_mean_R=1/0.6
        //   norm_flat = 0.4 * (1/0.6) = 0.6667
        //   result = 0.5 / 0.6667 = 0.75
        assert!(
            (light.data[0] - 0.75).abs() < 1e-4,
            "R(0,0): got {}",
            light.data[0]
        );

        // For R pixel at (2,0): flat=0.8, inv_mean_R=1/0.6
        //   norm_flat = 0.8 * (1/0.6) = 1.3333
        //   result = 0.5 / 1.3333 = 0.375
        assert!(
            (light.data[2] - 0.375).abs() < 1e-4,
            "R(2,0): got {}",
            light.data[2]
        );

        // For G pixel at (1,0): flat=0.6, inv_mean_G=1/0.6
        //   norm_flat = 0.6 * (1/0.6) = 1.0
        //   result = 0.5 / 1.0 = 0.5
        assert!(
            (light.data[1] - 0.5).abs() < 1e-4,
            "G(1,0): got {}",
            light.data[1]
        );

        // For B pixel at (1,1): flat=0.3, inv_mean_B=1/0.3
        //   norm_flat = 0.3 * (1/0.3) = 1.0
        //   result = 0.5 / 1.0 = 0.5
        assert!(
            (light.data[5] - 0.5).abs() < 1e-4,
            "B(1,1): got {}",
            light.data[5]
        );
    }

    #[test]
    fn test_bayer_flat_with_bias_per_channel() {
        // 4×4 RGGB, flat with bias subtraction, per-channel means.
        //
        // Flat = uniform 0.8 everywhere
        // Bias: R pixels = 0.2, G pixels = 0.1, B pixels = 0.05
        //
        // flat - bias per channel:
        //   R: 0.8 - 0.2 = 0.6
        //   G: 0.8 - 0.1 = 0.7
        //   B: 0.8 - 0.05 = 0.75
        //
        // Per-channel means: mean_R=0.6, mean_G=0.7, mean_B=0.75
        // norm_flat for each pixel = (flat-bias) / mean_color = 1.0 for all
        // So uniform light should be unchanged.
        let cfa = CfaType::Bayer(CfaPattern::Rggb);

        let mut bias_pixels = vec![0.0f32; 16];
        for y in 0..4 {
            for x in 0..4 {
                let color = cfa.color_at(x, y);
                bias_pixels[y * 4 + x] = match color {
                    0 => 0.2,
                    1 => 0.1,
                    2 => 0.05,
                    _ => unreachable!(),
                };
            }
        }

        let mut light = make_cfa(4, 4, vec![0.5; 16], cfa.clone());
        let flat = make_cfa(4, 4, vec![0.8; 16], cfa.clone());
        let bias = make_cfa(4, 4, bias_pixels, cfa);

        light.divide_by_normalized(&flat, Some(&bias));

        for &v in light.data.pixels() {
            assert!((v - 0.5).abs() < 1e-5, "Expected 0.5, got {v}");
        }
    }

    #[test]
    fn test_mono_flat_still_uses_global_mean() {
        // Mono CFA should use the old single-mean path, not per-channel.
        // This is a regression test ensuring the refactor didn't break mono.
        //
        // flat = [0.5, 1.0, 1.0, 0.5], mean = 0.75
        // norm_flat = [0.667, 1.333, 1.333, 0.667]
        // light = [0.25, 0.5, 0.5, 0.25]
        // result = [0.375, 0.375, 0.375, 0.375]
        let mut light = make_cfa(2, 2, vec![0.25, 0.5, 0.5, 0.25], CfaType::Mono);
        let flat = make_cfa(2, 2, vec![0.5, 1.0, 1.0, 0.5], CfaType::Mono);

        light.divide_by_normalized(&flat, None);

        assert!((light.data[0] - 0.375).abs() < 1e-4);
        assert!((light.data[1] - 0.375).abs() < 1e-4);
        assert!((light.data[2] - 0.375).abs() < 1e-4);
        assert!((light.data[3] - 0.375).abs() < 1e-4);
    }

    #[test]
    fn test_bayer_flat_corrects_color_shift() {
        // Key test: with single-mean, non-white flat introduces color shift.
        // With per-channel means, color shift is corrected.
        //
        // 2×2 RGGB: R=0.9, G=0.6, B=0.3
        //   Single global mean = (0.9 + 0.6 + 0.6 + 0.3) / 4 = 0.6
        //   With single mean: norm_R = 0.9/0.6 = 1.5, norm_G = 0.6/0.6 = 1.0, norm_B = 0.3/0.6 = 0.5
        //   Light = [0.5, 0.5, 0.5, 0.5]
        //   Single-mean result: [0.5/1.5, 0.5/1.0, 0.5/1.0, 0.5/0.5] = [0.333, 0.5, 0.5, 1.0]
        //   → R is dimmed, B is boosted — COLOR SHIFT!
        //
        //   Per-channel means: mean_R=0.9, mean_G=0.6, mean_B=0.3
        //   norm_R = 0.9/0.9 = 1.0, norm_G = 0.6/0.6 = 1.0, norm_B = 0.3/0.3 = 1.0
        //   Per-channel result: [0.5, 0.5, 0.5, 0.5] — no color shift!
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        let flat_pixels = vec![0.9, 0.6, 0.6, 0.3]; // R, G, G, B
        let mut light = make_cfa(2, 2, vec![0.5; 4], cfa.clone());
        let flat = make_cfa(2, 2, flat_pixels, cfa);

        light.divide_by_normalized(&flat, None);

        // All pixels should be 0.5 (no color shift)
        for (i, &v) in light.data.pixels().iter().enumerate() {
            assert!((v - 0.5).abs() < 1e-5, "Pixel {i}: expected 0.5, got {v}");
        }
    }
}
