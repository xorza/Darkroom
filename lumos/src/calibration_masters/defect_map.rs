//! Hot pixel detection and correction.
//!
//! Detects defective sensor pixels from master dark frames and corrects them.
//!
//! # Algorithm
//!
//! Uses **Median Absolute Deviation (MAD)** for robust σ estimation:
//!
//! 1. **Why MAD instead of standard deviation?**
//!    Standard deviation is heavily influenced by outliers - the very pixels we're
//!    trying to detect. MAD is robust: even if 49% of pixels are outliers, the
//!    median (and thus MAD) remains accurate.
//!
//! 2. **The 1.4826 constant (MAD to σ conversion):**
//!    For a normal distribution, MAD ≈ 0.6745 × σ. Therefore σ ≈ 1.4826 × MAD.
//!    This constant comes from the inverse of the 75th percentile of the standard
//!    normal distribution: 1/Φ⁻¹(0.75) ≈ 1.4826.
//!
//! 3. **CFA-aware correction:**
//!    On raw CFA data, hot pixels are replaced with the median of same-color
//!    neighbors (e.g., for Bayer, the nearest pixels of the same R/G/B filter).
//!    This preserves the CFA pattern for subsequent demosaicing.
//!
//! 4. **Adaptive sampling for large images:**
//!    For images >200K pixels, exact median computation is slow. We sample 100K
//!    pixels uniformly, which gives <0.5% median error with >99% confidence
//!    (by the central limit theorem for order statistics).

use crate::astro_image::cfa::{CfaImage, CfaType};
use common::Buffer2;

use arrayvec::ArrayVec;

/// A mask of defective pixels: **hot** pixels (abnormally high dark current) from a master
/// dark, and **cold/dead** pixels (abnormally low response) from a master flat.
///
/// Each is detected per CFA color and replaced with the median of same-color neighbors during
/// correction. The two defects come from *different* masters by necessity: a dark has no
/// illumination, so dead pixels are invisible in it (they read the same near-zero as a normal
/// dark pixel) — they only reveal themselves as dark spots in an illuminated flat.
#[derive(Debug, Clone)]
pub struct DefectMap {
    /// Flat indices of hot pixels (above `median + kσ` in the dark), ascending.
    pub hot_indices: Vec<usize>,
    /// Flat indices of cold/dead pixels (below `median − kσ` in the flat), ascending.
    pub cold_indices: Vec<usize>,
    pub width: usize,
    pub height: usize,
}

impl DefectMap {
    /// Build a defect map from the available masters: **hot** pixels from the dark and
    /// **cold/dead** pixels from the flat. Returns `None` if neither master is present.
    ///
    /// This is the constructor calibration uses; [`from_master_dark`](Self::from_master_dark)
    /// and [`from_master_flat`](Self::from_master_flat) build the halves individually.
    pub fn from_masters(
        dark: Option<&CfaImage>,
        flat: Option<&CfaImage>,
        sigma_threshold: f32,
    ) -> Option<Self> {
        let reference = dark.or(flat)?;
        let width = reference.data.width();
        let height = reference.data.height();
        if let (Some(d), Some(f)) = (dark, flat) {
            assert!(
                d.data.width() == f.data.width() && d.data.height() == f.data.height(),
                "master dark and flat must share dimensions"
            );
        }

        let hot_indices = dark.map_or_else(Vec::new, |d| {
            detect_defects(d, sigma_threshold, DefectSide::Hot)
        });
        let cold_indices = flat.map_or_else(Vec::new, |f| {
            detect_defects(f, sigma_threshold, DefectSide::Cold)
        });

        Some(Self {
            hot_indices,
            cold_indices,
            width,
            height,
        })
    }

    /// Detect **hot** pixels from a raw CFA master dark — those above `median + kσ` for their
    /// CFA color. Per-color stats keep green pixels (50% of Bayer data) from masking defects
    /// in the red/blue channels.
    pub fn from_master_dark(dark: &CfaImage, sigma_threshold: f32) -> Self {
        Self::from_masters(Some(dark), None, sigma_threshold).expect("dark is Some")
    }

    /// Detect **cold/dead** pixels from a raw CFA master flat — those below `median − kσ` for
    /// their CFA color. A dead pixel reads near zero regardless of illumination, so it sits
    /// far below the flat's (illuminated) per-color level. The threshold is *not* clamped to
    /// zero: for a flat the per-color median is a positive light level, so `median − kσ` is
    /// already a sensible positive cutoff that stays above a normal (vignetted) pixel.
    pub fn from_master_flat(flat: &CfaImage, sigma_threshold: f32) -> Self {
        Self::from_masters(None, Some(flat), sigma_threshold).expect("flat is Some")
    }

    /// Total number of defective pixels (hot + cold).
    pub fn count(&self) -> usize {
        self.hot_indices.len() + self.cold_indices.len()
    }

    /// Get the percentage of defective pixels.
    pub fn percentage(&self) -> f32 {
        let pixel_count = self.width * self.height;
        100.0 * self.count() as f32 / pixel_count as f32
    }

    /// Correct defective pixels on raw CFA data by replacing with median of
    /// same-color CFA neighbors.
    pub fn correct(&self, image: &mut CfaImage) {
        assert!(
            image.data.width() == self.width && image.data.height() == self.height,
            "CfaImage dimensions {}x{} don't match defect pixel map {}x{}",
            image.data.width(),
            image.data.height(),
            self.width,
            self.height
        );

        if self.hot_indices.is_empty() && self.cold_indices.is_empty() {
            return;
        }

        let cfa_type = image
            .metadata
            .cfa_type
            .as_ref()
            .expect("image must have CFA type for defect correction");

        for &idx in self.hot_indices.iter().chain(&self.cold_indices) {
            let x = idx % image.data.width();
            let y = idx / image.data.width();
            image.data[idx] = median_same_color_neighbors(&image.data, x, y, cfa_type);
        }
    }
}

/// Maximum number of samples per color channel for median estimation.
const MAX_MEDIAN_SAMPLES: usize = 100_000;

use crate::math::statistics::MAD_TO_SIGMA;

/// Get CFA color index at (x, y). Returns 0 for Mono (None CFA type).
fn cfa_color_at(cfa_type: Option<&CfaType>, x: usize, y: usize) -> u8 {
    match cfa_type {
        Some(cfa) => cfa.color_at(x, y),
        // Mono images have no CFA pattern — treat all pixels as the same color channel.
        None => 0,
    }
}

/// Which tail of the per-color distribution flags a defect.
#[derive(Debug, Clone, Copy)]
enum DefectSide {
    /// Hot pixels: above `median + kσ` (from a master dark).
    Hot,
    /// Cold/dead pixels: below `median − kσ` (from a master flat).
    Cold,
}

/// Flag the flat indices of pixels in the `side` tail, tested per CFA color.
fn detect_defects(image: &CfaImage, sigma_threshold: f32, side: DefectSide) -> Vec<usize> {
    assert!(sigma_threshold > 0.0, "Sigma threshold must be positive");
    let data = &image.data;
    let width = data.width();
    let cfa_type = image.metadata.cfa_type.as_ref();
    let stats = compute_per_color_stats(data, cfa_type);

    let mut indices = Vec::new();
    for (i, &val) in data.iter().enumerate() {
        let color = cfa_color_at(cfa_type, i % width, i / width) as usize;
        let (median, sigma) = stats[color];
        let flagged = match side {
            DefectSide::Hot => val > median + sigma_threshold * sigma,
            DefectSide::Cold => val < median - sigma_threshold * sigma,
        };
        if flagged {
            indices.push(i);
        }
    }
    indices
}

/// Per-CFA-color robust `(median, sigma)`, indexed by color (0=R/mono, 1=G, 2=B).
///
/// `sigma` is `MAD · 1.4826` with two floors that keep a clean (near-uniform) master from
/// flagging its own noise tail: an absolute floor (`5e-4`, ~33 ADU in 16-bit) and a relative
/// floor (`median · 0.1`). A color with no samples gets `sigma = ∞` so it never flags.
fn compute_per_color_stats(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
) -> ArrayVec<(f32, f32), 3> {
    let num_colors = cfa_type.map_or(1, |c| c.num_colors());
    let mut stats = ArrayVec::new();

    for color in 0..num_colors as u8 {
        let mut samples = collect_color_samples(data, cfa_type, color);

        if samples.is_empty() {
            stats.push((0.0, f32::INFINITY));
            continue;
        }

        let median = crate::math::statistics::median_f32_mut(&mut samples);
        for v in samples.iter_mut() {
            *v = (*v - median).abs();
        }
        let mad = crate::math::statistics::median_f32_mut(&mut samples);

        let sigma = (mad * MAD_TO_SIGMA).max(median * 0.1).max(5e-4);

        tracing::info!(
            "Defect stats color={color}: median={median:.6}, MAD={mad:.6}, sigma={sigma:.6}"
        );
        stats.push((median, sigma));
    }

    stats
}

/// Collect pixel samples for a specific CFA color channel.
///
/// Uses uniform sampling when the channel has more than `MAX_MEDIAN_SAMPLES * 2` pixels.
fn collect_color_samples(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    target_color: u8,
) -> Vec<f32> {
    let width = data.width();
    let height = data.height();
    let total = width * height;

    if cfa_type.map_or(1, |c| c.num_colors()) == 1 {
        // Mono: all pixels belong to color 0, use strided sampling
        let use_sampling = total > MAX_MEDIAN_SAMPLES * 2;
        let sample_count = if use_sampling {
            MAX_MEDIAN_SAMPLES
        } else {
            total
        };
        let stride = if use_sampling {
            total / sample_count
        } else {
            1
        };
        return (0..sample_count).map(|i| data[i * stride]).collect();
    }

    // CFA: collect all pixels of target_color, then subsample if too many
    let mut pixels: Vec<f32> = (0..total)
        .filter(|&i| cfa_color_at(cfa_type, i % width, i / width) == target_color)
        .map(|i| data[i])
        .collect();

    if pixels.len() > MAX_MEDIAN_SAMPLES * 2 {
        let stride = pixels.len() / MAX_MEDIAN_SAMPLES;
        pixels = (0..MAX_MEDIAN_SAMPLES)
            .map(|i| pixels[i * stride])
            .collect();
    }

    pixels
}

/// Calculate median of 8-connected neighbors from raw channel data.
fn median_of_neighbors_raw(pixels: &Buffer2<f32>, x: usize, y: usize) -> f32 {
    let width = pixels.width();
    let height = pixels.height();
    let mut neighbors: [f32; 8] = [0.0; 8];
    let mut count = 0;

    let offsets: [(i32, i32); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    for (dx, dy) in offsets {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;

        if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
            neighbors[count] = *pixels.get(nx as usize, ny as usize);
            count += 1;
        }
    }

    if count == 0 {
        return *pixels.get(x, y);
    }

    crate::math::statistics::median_f32_mut(&mut neighbors[..count])
}

/// Find same-color CFA neighbors and return their median.
///
/// For Bayer patterns, same-color neighbors are at stride-2 offsets.
/// For X-Trans, searches within a radius of 2*period.
/// For Mono, uses standard 8-connected neighbors.
fn median_same_color_neighbors(
    pixels: &Buffer2<f32>,
    x: usize,
    y: usize,
    pattern: &CfaType,
) -> f32 {
    match pattern {
        CfaType::Mono => median_of_neighbors_raw(pixels, x, y),
        CfaType::Bayer(_) => bayer_same_color_median(pixels, x, y),
        CfaType::XTrans(_) => xtrans_same_color_median(pixels, x, y, pattern),
    }
}

/// Optimized Bayer same-color neighbor median.
/// Same-color neighbors are at stride 2 in all directions.
fn bayer_same_color_median(pixels: &Buffer2<f32>, x: usize, y: usize) -> f32 {
    let width = pixels.width();
    let height = pixels.height();
    let offsets: [(i32, i32); 8] = [
        (-2, 0),
        (2, 0),
        (0, -2),
        (0, 2),
        (-2, -2),
        (-2, 2),
        (2, -2),
        (2, 2),
    ];
    let mut buf = [0.0f32; 8];
    let mut count = 0;
    for (dx, dy) in offsets {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
            buf[count] = *pixels.get(nx as usize, ny as usize);
            count += 1;
        }
    }
    if count == 0 {
        return *pixels.get(x, y);
    }
    crate::math::statistics::median_f32_mut(&mut buf[..count])
}

/// X-Trans same-color neighbor median.
/// Searches within radius of 6 (one full period) for same-color pixels.
/// Collects all same-color neighbors, sorts by Manhattan distance, and takes
/// the closest 24 to avoid directional bias.
fn xtrans_same_color_median(pixels: &Buffer2<f32>, x: usize, y: usize, pattern: &CfaType) -> f32 {
    let width = pixels.width();
    let height = pixels.height();
    let my_color = pattern.color_at(x, y);

    let radius = 6i32;

    // Collect all same-color neighbors with their Manhattan distance
    let mut candidates: ArrayVec<(i32, f32), 169> = ArrayVec::new(); // 13×13 = 169 max

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let nx = nx as usize;
            let ny = ny as usize;
            if pattern.color_at(nx, ny) == my_color {
                let dist = dx.abs() + dy.abs();
                candidates.push((dist, *pixels.get(nx, ny)));
            }
        }
    }

    if candidates.is_empty() {
        return pixels.row(y)[x];
    }

    // Sort by Manhattan distance, take closest 24.
    // 24 ≈ 4 per cardinal/diagonal direction in the 6×6 X-Trans pattern,
    // enough for a robust median without directional bias.
    candidates.sort_unstable_by_key(|&(dist, _)| dist);
    let n = candidates.len().min(24);
    let mut neighbors = [0.0f32; 24];
    for (i, &(_, val)) in candidates[..n].iter().enumerate() {
        neighbors[i] = val;
    }
    crate::math::statistics::median_f32_mut(&mut neighbors[..n])
}

/// Per-class defect counts, used only by tests to assert detection behavior.
#[cfg(test)]
impl DefectMap {
    /// Number of hot pixels detected.
    pub fn hot_count(&self) -> usize {
        self.hot_indices.len()
    }

    /// Number of cold/dead pixels detected.
    pub fn cold_count(&self) -> usize {
        self.cold_indices.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::make_cfa;

    fn is_hot(defect_map: &DefectMap, pixel_idx: usize) -> bool {
        defect_map.hot_indices.binary_search(&pixel_idx).is_ok()
    }

    fn is_cold(defect_map: &DefectMap, pixel_idx: usize) -> bool {
        defect_map.cold_indices.binary_search(&pixel_idx).is_ok()
    }

    #[test]
    fn test_cfa_hot_pixel_detection() {
        // 6x6 CFA image with known hot pixels
        let mut pixels = vec![100.0; 36];
        pixels[0] = 10000.0; // hot at (0,0)
        pixels[14] = 10000.0; // hot at (2,2)
        pixels[35] = 10000.0; // hot at (5,5)

        let dark = make_cfa(
            6,
            6,
            pixels,
            CfaType::Bayer(crate::raw::demosaic::bayer::CfaPattern::Rggb),
        );
        let defect_map = DefectMap::from_master_dark(&dark, 5.0);

        assert_eq!(defect_map.hot_count(), 3);
        assert!(is_hot(&defect_map, 0));
        assert!(is_hot(&defect_map, 14));
        assert!(is_hot(&defect_map, 35));
        assert!(!is_hot(&defect_map, 1)); // not hot
    }

    #[test]
    fn test_cfa_hot_pixel_correction_bayer() {
        // 6x6 Bayer RGGB pattern
        // Hot pixel at (2,2) = R. Same-color (R) neighbors at stride 2.
        let mut pixels = vec![100.0; 36];
        pixels[2 * 6 + 2] = 10000.0; // hot at (2,2)

        let mut image = make_cfa(
            6,
            6,
            pixels,
            CfaType::Bayer(crate::raw::demosaic::bayer::CfaPattern::Rggb),
        );

        let defect_map = DefectMap {
            hot_indices: vec![2 * 6 + 2],
            cold_indices: vec![],
            width: 6,
            height: 6,
        };

        defect_map.correct(&mut image);

        // Should be replaced with median of same-color neighbors (all 100.0)
        assert!(
            (image.data[2 * 6 + 2] - 100.0).abs() < f32::EPSILON,
            "Expected 100.0, got {}",
            image.data[2 * 6 + 2]
        );
    }

    #[test]
    fn test_cfa_hot_pixel_correction_mono() {
        // Mono: uses standard 8-connected neighbors
        let pixels = vec![10.0, 20.0, 30.0, 40.0, 1000.0, 50.0, 60.0, 70.0, 80.0];
        let mut image = make_cfa(3, 3, pixels, CfaType::Mono);

        let defect_map = DefectMap {
            hot_indices: vec![4],
            cold_indices: vec![],
            width: 3,
            height: 3,
        };

        defect_map.correct(&mut image);

        // Median of [10, 20, 30, 40, 50, 60, 70, 80] = 45
        assert!(
            (image.data[4] - 45.0).abs() < f32::EPSILON,
            "Expected 45.0, got {}",
            image.data[4]
        );
    }

    #[test]
    fn test_bayer_same_color_neighbors() {
        // 6x6 image, all 100.0, hot pixel at center (2,2)
        let mut pixels = vec![100.0; 36];
        // Set some same-color neighbors to distinct values to verify median
        pixels[0] = 50.0; // (0,0)
        pixels[4] = 60.0; // (4,0)
        pixels[2] = 70.0; // (2,0)
        pixels[4 * 6 + 2] = 80.0; // (2,4)

        let pixels = common::Buffer2::new(6, 6, pixels);
        let result = bayer_same_color_median(&pixels, 2, 2);

        // Neighbors: 50, 60, 70, 80, 100 (0,2=100), 100 (4,2=100), 100 (0,4=100), 100 (4,4=100)
        // Sorted: 50, 60, 70, 80, 100, 100, 100, 100 → median of 8 = (80+100)/2 = 90
        assert!(
            (result - 90.0).abs() < f32::EPSILON,
            "Expected 90.0, got {}",
            result
        );
    }

    #[test]
    fn test_bayer_same_color_neighbors_corner() {
        // Hot pixel at corner (0,0) in 4x4 Bayer RGGB
        // Same-color (R) neighbors at stride 2: (2,0), (0,2), (2,2)
        let pixels = vec![
            999.0, 10.0, 50.0, 10.0, 10.0, 10.0, 10.0, 10.0, 60.0, 10.0, 70.0, 10.0, 10.0, 10.0,
            10.0, 10.0,
        ];
        let pixels = common::Buffer2::new(4, 4, pixels);
        let result = bayer_same_color_median(&pixels, 0, 0);

        // Same-color neighbors: (2,0)=50, (0,2)=60, (2,2)=70
        // Median of [50, 60, 70] = 60
        assert!(
            (result - 60.0).abs() < f32::EPSILON,
            "Expected 60.0, got {}",
            result
        );
    }

    #[test]
    fn test_cfa_hot_pixel_detection_large() {
        // Large enough to trigger sampling
        let size = 500;
        let pixel_count = size * size;
        let mut pixels = vec![100.0; pixel_count];

        let hot_positions = [0, 500, 5000, 50000, 100000, 200000, 249999];
        for &idx in &hot_positions {
            pixels[idx] = 10000.0;
        }

        let dark = make_cfa(
            size,
            size,
            pixels,
            CfaType::Bayer(crate::raw::demosaic::bayer::CfaPattern::Rggb),
        );
        let defect_map = DefectMap::from_master_dark(&dark, 5.0);

        assert_eq!(defect_map.hot_count(), hot_positions.len());
        for &idx in &hot_positions {
            assert!(
                is_hot(&defect_map, idx),
                "Hot pixel at {} not detected",
                idx
            );
        }
    }

    #[test]
    fn test_per_channel_detection_bayer() {
        // 8x8 Bayer RGGB image.
        // Red pixels (at even x, even y) have value 100.0
        // Green pixels have value 200.0
        // Blue pixels (at odd x, odd y) have value 50.0
        // One red pixel is hot at 500.0 — should be detected by per-channel stats
        // even though 500 might not exceed a global threshold dominated by green=200.
        let pattern = CfaType::Bayer(crate::raw::demosaic::bayer::CfaPattern::Rggb);
        let mut pixels = vec![0.0f32; 64];
        for y in 0..8 {
            for x in 0..8 {
                let color = pattern.color_at(x, y);
                pixels[y * 8 + x] = match color {
                    0 => 100.0, // R
                    1 => 200.0, // G
                    2 => 50.0,  // B
                    _ => unreachable!(),
                };
            }
        }
        // Make one red pixel hot
        pixels[0] = 500.0; // (0,0) = R

        let dark = make_cfa(8, 8, pixels, pattern.clone());
        let defect_map = DefectMap::from_master_dark(&dark, 3.0);

        // The hot red pixel should be detected
        assert!(
            is_hot(&defect_map, 0),
            "Hot red pixel at (0,0) not detected"
        );

        // Green and blue pixels should not be flagged
        assert!(!is_hot(&defect_map, 1)); // G at (1,0)
        assert!(!is_hot(&defect_map, 9)); // B at (1,1)
    }

    #[test]
    fn test_cfa_no_defective_pixels() {
        let pixels = vec![100.0; 36];
        let dark = make_cfa(6, 6, pixels, CfaType::Mono);
        let defect_map = DefectMap::from_master_dark(&dark, 5.0);
        assert_eq!(defect_map.hot_count(), 0);
        assert_eq!(defect_map.count(), 0);
    }

    /// Regression for the negative-median-dark bug: a master dark centered near zero (with a
    /// negative noise tail, as after bias subtraction) must NOT flag the sub-zero pixels as
    /// defects. Only the handful of genuinely hot pixels should be flagged (≪1%).
    #[test]
    fn near_zero_median_dark_flags_only_hot_pixels() {
        // 64x64 mono dark: small zero-mean noise (many pixels < 0), median ≈ 0.
        let (w, h) = (64usize, 64usize);
        let mut pixels: Vec<f32> = (0..w * h)
            .map(|i| if i % 2 == 0 { -0.0003 } else { 0.0002 })
            .collect();
        // Three genuinely hot pixels.
        for &idx in &[100usize, 2000, 4000] {
            pixels[idx] = 0.5;
        }
        let dark = make_cfa(w, h, pixels, CfaType::Mono);

        let defect_map = DefectMap::from_master_dark(&dark, 5.0);

        // Exactly the hot pixels — the ~half-the-frame sub-zero pixels are not "cold defects".
        assert_eq!(
            defect_map.count(),
            3,
            "only the hot pixels should be flagged"
        );
        assert!(is_hot(&defect_map, 100));
        assert!(is_hot(&defect_map, 2000));
        assert!(is_hot(&defect_map, 4000));
        assert!(
            defect_map.percentage() < 1.0,
            "defects should be ≪1%, got {:.2}%",
            defect_map.percentage()
        );
    }

    /// Cold/dead pixels come from the *flat* (illuminated), where a dead pixel is a dark spot.
    #[test]
    fn cold_pixels_detected_from_flat() {
        // 6x6 mono flat: uniform illumination 0.4 with one dead pixel (no response).
        let mut pixels = vec![0.4f32; 36];
        pixels[5] = 0.0; // dead pixel at index 5
        let flat = make_cfa(6, 6, pixels, CfaType::Mono);

        let defect_map = DefectMap::from_master_flat(&flat, 5.0);

        // median 0.4, sigma floored at 0.04 → cold threshold 0.4 − 5·0.04 = 0.2; only the dead
        // pixel (0.0) is below it. A flat yields no hot pixels.
        assert_eq!(defect_map.cold_count(), 1, "the dead pixel is cold");
        assert_eq!(defect_map.hot_count(), 0, "a flat yields no hot pixels");
        assert!(is_cold(&defect_map, 5));
        assert!(!is_cold(&defect_map, 0)); // a normal illuminated pixel
    }

    /// A clean (uniform) flat must not flag its own pixels as cold — the σ floor guards this.
    #[test]
    fn uniform_flat_flags_no_cold() {
        let flat = make_cfa(8, 8, vec![0.5f32; 64], CfaType::Mono);
        let defect_map = DefectMap::from_master_flat(&flat, 5.0);
        assert_eq!(defect_map.count(), 0);
    }

    /// `from_masters` merges hot pixels (from the dark) and cold pixels (from the flat).
    #[test]
    fn from_masters_combines_dark_hot_and_flat_cold() {
        // Near-zero dark with one hot pixel; illuminated flat with one dead pixel.
        let mut dark_px = vec![0.001f32; 36];
        dark_px[0] = 0.5; // hot
        let dark = make_cfa(6, 6, dark_px, CfaType::Mono);

        let mut flat_px = vec![0.4f32; 36];
        flat_px[5] = 0.0; // dead
        let flat = make_cfa(6, 6, flat_px, CfaType::Mono);

        let defect_map = DefectMap::from_masters(Some(&dark), Some(&flat), 5.0).unwrap();

        assert_eq!(defect_map.hot_count(), 1);
        assert_eq!(defect_map.cold_count(), 1);
        assert_eq!(defect_map.count(), 2);
        assert!(is_hot(&defect_map, 0));
        assert!(is_cold(&defect_map, 5));
    }

    #[test]
    #[should_panic(expected = "don't match")]
    fn test_correct_cfa_dimension_mismatch() {
        let pixels = vec![10.0; 9];
        let mut image = make_cfa(3, 3, pixels, CfaType::Mono);

        let defect_map = DefectMap {
            hot_indices: vec![],
            cold_indices: vec![],
            width: 2,
            height: 2,
        };

        defect_map.correct(&mut image);
    }
}
