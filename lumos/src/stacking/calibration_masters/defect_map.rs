//! Defective-pixel detection and correction.
//!
//! **Hot** pixels (abnormally high dark current) come from the master dark via a robust per-color
//! MAD threshold; **cold/dead** pixels (abnormally low response) come from the master flat via a
//! local-neighbourhood ratio test. Both are corrected by replacing the pixel with the median of
//! its same-color CFA neighbours.
//!
//! # Hot pixels (from the dark)
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
//!
//! # Cold/dead pixels (from the flat)
//!
//! A *global* threshold cannot find dead pixels in a real flat: vignetting spreads the per-color
//! values so wide that `median − kσ` falls below zero, so nothing is ever flagged. Instead a
//! pixel is dead when it reads below [`DEAD_PIXEL_FRACTION`] of the median of its *same-color
//! local neighbours* — a reference that tracks vignetting (smooth, locally flat) and ignores dust
//! shadows (which dim by far less than half), so only genuinely near-zero pixels are caught.

use crate::io::astro_image::cfa::{CfaImage, CfaType};
use crate::math::statistics::median_f32_mut;
use common::BitBuffer2;
use imaginarium::Buffer2;
use common::CancelToken;
use common::Vec2us;

use arrayvec::ArrayVec;
use rayon::prelude::*;

/// A mask of defective pixels: **hot** pixels (abnormally high dark current) from a master
/// dark, and **cold/dead** pixels (abnormally low response) from a master flat.
///
/// Each is detected per CFA color and replaced with the median of same-color neighbors during
/// correction. The two defects come from *different* masters by necessity: a dark has no
/// illumination, so dead pixels are invisible in it (they read the same near-zero as a normal
/// dark pixel) — they only reveal themselves as dark spots in an illuminated flat.
#[derive(Debug, Clone, Default)]
pub struct DefectMap {
    /// Flat indices of hot pixels (above `median + kσ` in the dark), ascending.
    pub hot_indices: Vec<usize>,
    /// Flat indices of cold/dead pixels (below [`DEAD_PIXEL_FRACTION`] of their same-color
    /// local-neighbourhood median in the flat), ascending.
    pub cold_indices: Vec<usize>,
    /// Sensor dimensions the indices apply to — `None` until the first `detect_*` call records them.
    dimensions: Option<Vec2us>,
}

impl DefectMap {
    /// Detect **hot** pixels from a master dark — those above `median + sigma_threshold·σ` for
    /// their CFA color — and store them. Chainable, in any order:
    /// `DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never()).detect_cold(&flat, &CancelToken::never())`.
    pub fn detect_hot(
        mut self,
        dark: &CfaImage,
        sigma_threshold: f32,
        cancel: &CancelToken,
    ) -> Self {
        // Clamp at the boundary rather than asserting: `sigma_threshold` may come from user config,
        // and a non-positive value (which would flag every pixel above the median) must not panic
        // the pipeline. Nothing below 1σ is a meaningful defect threshold.
        let sigma_threshold = sigma_threshold.max(MIN_SIGMA_THRESHOLD);
        self.set_dimensions(Vec2us::new(dark.data.width(), dark.data.height()));
        self.hot_indices = detect_hot_pixels(dark, sigma_threshold, cancel);
        self
    }

    /// Detect **cold/dead** pixels from a master flat — those reading below [`DEAD_PIXEL_FRACTION`]
    /// of their same-color local-neighbourhood median — and store them. The local reference makes
    /// this robust to vignetting and dust, where a global cut cannot be.
    pub fn detect_cold(mut self, flat: &CfaImage, cancel: &CancelToken) -> Self {
        self.set_dimensions(Vec2us::new(flat.data.width(), flat.data.height()));
        self.cold_indices = detect_cold_pixels(flat, DEAD_PIXEL_FRACTION, cancel);
        self
    }

    /// Record the master dimensions on first detection, or assert they match on later calls: every
    /// master feeding one map corrects the same sensor, so all must share dimensions.
    fn set_dimensions(&mut self, dims: Vec2us) {
        match self.dimensions {
            None => self.dimensions = Some(dims),
            Some(existing) => assert!(
                existing == dims,
                "all masters must share dimensions: have {existing:?}, got {dims:?}"
            ),
        }
    }

    /// Total number of defective pixels (hot + cold).
    pub fn count(&self) -> usize {
        self.hot_indices.len() + self.cold_indices.len()
    }

    /// Percentage of defective pixels, or `0.0` before any master has been detected.
    pub fn percentage(&self) -> f32 {
        self.dimensions
            .map_or(0.0, |d| 100.0 * self.count() as f32 / (d.x * d.y) as f32)
    }

    /// Correct defective pixels on raw CFA data by replacing with median of
    /// same-color CFA neighbors.
    pub fn correct(&self, image: &mut CfaImage) {
        let dims = self
            .dimensions
            .expect("defect map has no dimensions; detect a master first");
        assert!(
            image.data.width() == dims.x && image.data.height() == dims.y,
            "CfaImage dimensions {}x{} don't match defect pixel map {}x{}",
            image.data.width(),
            image.data.height(),
            dims.x,
            dims.y
        );

        if self.hot_indices.is_empty() && self.cold_indices.is_empty() {
            return;
        }

        let cfa_type = image
            .metadata
            .cfa_type
            .as_ref()
            .expect("image must have CFA type for defect correction");

        // Mask every defect so each repair draws only on GOOD neighbours. Without it, a clustered
        // defect (hot column, adjacent same-color pixels) pulls a neighbour's bad/half-corrected
        // value into its median and the order of `hot ⧺ cold` changes the result.
        let width = image.data.width();
        let mut mask = BitBuffer2::new_default(width, image.data.height());
        for &idx in self.hot_indices.iter().chain(&self.cold_indices) {
            mask.set(idx, true);
        }

        for &idx in self.hot_indices.iter().chain(&self.cold_indices) {
            let x = idx % width;
            let y = idx / width;
            image.data[idx] = median_same_color_neighbors(&image.data, x, y, cfa_type, Some(&mask));
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

/// Lowest hot-pixel σ multiplier `detect_hot` will honor. A non-positive (or absurdly small)
/// threshold would flag a huge fraction of the sensor; clamping here keeps a mis-set user config
/// from panicking or wiping the frame.
const MIN_SIGMA_THRESHOLD: f32 = 1.0;

/// A flat pixel reading below this fraction of its same-color local-neighbourhood median is
/// treated as dead. 0.5 ("less than half the local response") sits well below vignetting (smooth,
/// locally flat) and dust shadows (which dim by far less), so only genuinely near-zero pixels are
/// flagged.
const DEAD_PIXEL_FRACTION: f32 = 0.5;

/// Flag hot pixels in a master dark: those above `median + kσ` for their CFA color (robust
/// per-color MAD stats). Per-color keeps green (50% of Bayer data) from masking red/blue defects.
fn detect_hot_pixels(image: &CfaImage, sigma_threshold: f32, cancel: &CancelToken) -> Vec<usize> {
    let data = &image.data;
    let width = data.width();
    let total = width * data.height();
    let cfa_type = image.metadata.cfa_type.as_ref();
    let stats = compute_per_color_stats(data, cfa_type);

    // Parallel like `detect_cold_pixels`: indexed `collect` keeps the result ascending, preserving
    // the `binary_search` invariant the map relies on.
    (0..total)
        .into_par_iter()
        .filter(|&i| {
            // Cancelled: skip the rest (partial result is discarded by the caller).
            if cancel.is_cancelled() {
                return false;
            }
            let color = cfa_color_at(cfa_type, i % width, i / width) as usize;
            let (median, sigma) = stats[color];
            data[i] > median + sigma_threshold * sigma
        })
        .collect()
}

/// Flag cold/dead pixels in a master flat: those reading below `dead_fraction` of the median of
/// their same-color local neighbours. The local reference tracks vignetting (so a global cut's
/// negative-threshold failure can't happen) and ignores dust shadows; only near-zero pixels pass.
/// The neighbour scan runs on every pixel in parallel — one-time work, off the hot path.
fn detect_cold_pixels(image: &CfaImage, dead_fraction: f32, cancel: &CancelToken) -> Vec<usize> {
    let data = &image.data;
    let width = data.width();
    let total = width * data.height();
    let cfa = image.metadata.cfa_type.clone().unwrap_or(CfaType::Mono);

    (0..total)
        .into_par_iter()
        .filter(|&i| {
            // Cancelled: skip the rest (partial result is discarded by the caller).
            if cancel.is_cancelled() {
                return false;
            }
            let local = median_same_color_neighbors(data, i % width, i / width, &cfa, None);
            data[i] < dead_fraction * local
        })
        .collect()
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

        let median = median_f32_mut(&mut samples);
        for v in samples.iter_mut() {
            *v = (*v - median).abs();
        }
        let mad = median_f32_mut(&mut samples);

        let sigma = (mad * MAD_TO_SIGMA).max(median * 0.1).max(5e-4);

        tracing::debug!(
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

    // CFA: stride-sample this color in a single pass so we never materialize all of its
    // pixels — the old "collect every matching pixel, then subsample" path allocated tens of MB
    // of throwaway. `keep_stride` from the pixel total (color count ≤ total) keeps well under
    // `MAX_MEDIAN_SAMPLES` samples, ample for a robust median/MAD.
    let keep_stride = (total / MAX_MEDIAN_SAMPLES).max(1);
    let mut samples = Vec::with_capacity(MAX_MEDIAN_SAMPLES.min(total));
    let mut seen = 0usize;
    for y in 0..height {
        for (x, &val) in data.row(y).iter().enumerate() {
            if cfa_color_at(cfa_type, x, y) == target_color {
                if seen.is_multiple_of(keep_stride) {
                    samples.push(val);
                }
                seen += 1;
            }
        }
    }
    samples
}

/// Calculate median of 8-connected neighbors from raw channel data, skipping any flagged in
/// `defect_mask` so a defect is never repaired from another defect.
fn median_of_neighbors_raw(
    pixels: &Buffer2<f32>,
    x: usize,
    y: usize,
    defect_mask: Option<&BitBuffer2>,
) -> f32 {
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
            let (nx, ny) = (nx as usize, ny as usize);
            if defect_mask.is_some_and(|m| m.get_xy(nx, ny)) {
                continue;
            }
            neighbors[count] = *pixels.get(nx, ny);
            count += 1;
        }
    }

    if count == 0 {
        return *pixels.get(x, y);
    }

    median_f32_mut(&mut neighbors[..count])
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
    defect_mask: Option<&BitBuffer2>,
) -> f32 {
    match pattern {
        CfaType::Mono => median_of_neighbors_raw(pixels, x, y, defect_mask),
        CfaType::Bayer(_) => bayer_same_color_median(pixels, x, y, defect_mask),
        CfaType::XTrans(_) => xtrans_same_color_median(pixels, x, y, pattern, defect_mask),
    }
}

/// Optimized Bayer same-color neighbor median, skipping neighbors flagged in `defect_mask`.
/// Same-color neighbors are at stride 2 in all directions.
fn bayer_same_color_median(
    pixels: &Buffer2<f32>,
    x: usize,
    y: usize,
    defect_mask: Option<&BitBuffer2>,
) -> f32 {
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
            let (nx, ny) = (nx as usize, ny as usize);
            if defect_mask.is_some_and(|m| m.get_xy(nx, ny)) {
                continue;
            }
            buf[count] = *pixels.get(nx, ny);
            count += 1;
        }
    }
    if count == 0 {
        return *pixels.get(x, y);
    }
    median_f32_mut(&mut buf[..count])
}

/// X-Trans same-color neighbor median.
/// Searches within radius of 6 (one full period) for same-color pixels.
/// Collects all same-color neighbors, sorts by Manhattan distance, and takes
/// the closest 24 to avoid directional bias.
fn xtrans_same_color_median(
    pixels: &Buffer2<f32>,
    x: usize,
    y: usize,
    pattern: &CfaType,
    defect_mask: Option<&BitBuffer2>,
) -> f32 {
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
            if defect_mask.is_some_and(|m| m.get_xy(nx, ny)) {
                continue;
            }
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
    median_f32_mut(&mut neighbors[..n])
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
mod bench {
    use super::*;
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use ::quickbench::quick_bench;

    #[quick_bench(warmup_iters = 3, iters = 20)]
    fn bench_collect_color_samples(b: quickbench::Bencher) {
        let (w, h) = (6000, 4000);
        let data = Buffer2::new(w, h, (0..w * h).map(|i| (i % 1000) as f32).collect());
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        b.bench(|| {
            std::hint::black_box(collect_color_samples(
                std::hint::black_box(&data),
                Some(&cfa),
                0,
            ))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{io::raw::demosaic::bayer::CfaPattern, testing::make_cfa};

    fn is_hot(defect_map: &DefectMap, pixel_idx: usize) -> bool {
        defect_map.hot_indices.binary_search(&pixel_idx).is_ok()
    }

    fn is_cold(defect_map: &DefectMap, pixel_idx: usize) -> bool {
        defect_map.cold_indices.binary_search(&pixel_idx).is_ok()
    }

    #[test]
    fn test_correct_clustered_defect_uses_only_good_neighbors() {
        // A defect whose same-color neighbours are MOSTLY other defects must still be repaired from
        // the few good ones — the defect mask excludes the bad neighbours. Pre-mask, the neighbour
        // median was dominated by the cluster and left the pixel ~uncorrected (≈0.95 here).
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        let (w, h) = (16usize, 16usize);
        // Red pixels sit at (even,even). Target (6,6); make 5 of its 8 stride-2 red neighbours hot.
        let hot = [(6, 6), (4, 6), (8, 6), (6, 4), (6, 8), (4, 4)];
        let mut px = vec![0.5f32; w * h];
        for &(x, y) in &hot {
            px[y * w + x] = 0.95;
        }
        let dark = make_cfa(w, h, px, cfa.clone());
        let defect_map = DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never());
        assert_eq!(defect_map.hot_count(), 6, "all six 0.95 red pixels are hot");

        let mut light = make_cfa(w, h, vec![0.5f32; w * h], cfa);
        for &(x, y) in &hot {
            light.data[y * w + x] = 0.95;
        }
        defect_map.correct(&mut light);

        // (6,6)'s only good red neighbours (4,8),(8,4),(8,8) are all 0.5 → median 0.5, despite the
        // five hot neighbours that would otherwise dominate.
        let corrected = light.data[6 * w + 6];
        assert!(
            (corrected - 0.5).abs() < 1e-4,
            "clustered defect repaired from good neighbours → expected 0.5, got {corrected}"
        );
    }

    #[test]
    fn test_xtrans_hot_pixel_correction_uses_same_color() {
        // X-Trans hot pixels must be repaired from SAME-COLOR neighbours, not the global mean.
        let pattern = [
            [1, 0, 1, 1, 2, 1],
            [2, 1, 2, 0, 1, 0],
            [1, 2, 1, 1, 0, 1],
            [1, 2, 1, 1, 0, 1],
            [0, 1, 0, 2, 1, 2],
            [1, 0, 1, 1, 2, 1],
        ];
        let cfa = CfaType::XTrans(pattern);
        let (w, h) = (12usize, 12usize);
        // Distinct per-color baselines so a wrong-color repair is detectable.
        let color_val = |c: u8| match c {
            0 => 0.1, // R
            1 => 0.2, // G
            _ => 0.3, // B
        };
        let build = |corrupt: &[(usize, usize)]| {
            let mut px = vec![0.0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    px[y * w + x] = color_val(cfa.color_at(x, y));
                }
            }
            for &(x, y) in corrupt {
                px[y * w + x] = 0.9;
            }
            make_cfa(w, h, px, cfa.clone())
        };

        let r_hot = (1usize, 0usize); // pattern[0][1] = 0 → R
        let b_hot = (0usize, 1usize); // pattern[1][0] = 2 → B
        assert_eq!(cfa.color_at(r_hot.0, r_hot.1), 0);
        assert_eq!(cfa.color_at(b_hot.0, b_hot.1), 2);

        let dark = build(&[r_hot, b_hot]);
        let defect_map = DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never());
        assert_eq!(defect_map.hot_count(), 2, "one R and one B hot pixel");

        let mut light = build(&[r_hot, b_hot]);
        defect_map.correct(&mut light);

        let r_val = light.data[r_hot.1 * w + r_hot.0];
        let b_val = light.data[b_hot.1 * w + b_hot.0];
        assert!(
            (r_val - 0.1).abs() < 1e-4,
            "R hot repaired from R neighbours → expected 0.1, got {r_val}"
        );
        assert!(
            (b_val - 0.3).abs() < 1e-4,
            "B hot repaired from B neighbours → expected 0.3, got {b_val}"
        );
    }

    #[test]
    fn test_cfa_hot_pixel_detection() {
        // 6x6 CFA image with known hot pixels
        let mut pixels = vec![100.0; 36];
        pixels[0] = 10000.0; // hot at (0,0)
        pixels[14] = 10000.0; // hot at (2,2)
        pixels[35] = 10000.0; // hot at (5,5)

        let dark = make_cfa(6, 6, pixels, CfaType::Bayer(CfaPattern::Rggb));
        let defect_map = DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never());

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

        let mut image = make_cfa(6, 6, pixels, CfaType::Bayer(CfaPattern::Rggb));

        let defect_map = DefectMap {
            hot_indices: vec![2 * 6 + 2],
            cold_indices: vec![],
            dimensions: Some(Vec2us::new(6, 6)),
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
            dimensions: Some(Vec2us::new(3, 3)),
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

        let pixels = imaginarium::Buffer2::new(6, 6, pixels);
        let result = bayer_same_color_median(&pixels, 2, 2, None);

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
        let pixels = imaginarium::Buffer2::new(4, 4, pixels);
        let result = bayer_same_color_median(&pixels, 0, 0, None);

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

        let dark = make_cfa(size, size, pixels, CfaType::Bayer(CfaPattern::Rggb));
        let defect_map = DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never());

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
        let pattern = CfaType::Bayer(CfaPattern::Rggb);
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
        let defect_map = DefectMap::default().detect_hot(&dark, 3.0, &CancelToken::never());

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
        let defect_map = DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never());
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

        let defect_map = DefectMap::default().detect_hot(&dark, 5.0, &CancelToken::never());

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

        let defect_map = DefectMap::default().detect_cold(&flat, &CancelToken::never());

        // The dead pixel (0.0) reads below half its uniform 0.4 neighbourhood (0.5·0.4 = 0.2);
        // every normal pixel reads its full 0.4. A flat yields no hot pixels.
        assert_eq!(defect_map.cold_count(), 1, "the dead pixel is cold");
        assert_eq!(defect_map.hot_count(), 0, "a flat yields no hot pixels");
        assert!(is_cold(&defect_map, 5));
        assert!(!is_cold(&defect_map, 0)); // a normal illuminated pixel
    }

    /// A clean (uniform) flat must not flag its own pixels as cold — every pixel equals its
    /// neighbourhood median, so none falls below half of it.
    #[test]
    fn uniform_flat_flags_no_cold() {
        let flat = make_cfa(8, 8, vec![0.5f32; 64], CfaType::Mono);
        let defect_map = DefectMap::default().detect_cold(&flat, &CancelToken::never());
        assert_eq!(defect_map.count(), 0);
    }

    /// The point of the local test: cold detection must survive a vignetted flat. A steep
    /// illumination gradient (0.2 → 0.8 across the frame, mimicking vignetting) defeats any
    /// *global* cut — `median − 5σ` goes negative (under-detects), while `0.5 × global_median =
    /// 0.25` wrongly flags the whole dim edge (0.2 < 0.25). The local-neighbour ratio flags only
    /// the genuinely dead pixel and leaves both edges alone.
    #[test]
    fn cold_detection_survives_vignetting_gradient() {
        let (w, h) = (16usize, 16usize);
        let mut pixels: Vec<f32> = (0..w * h)
            .map(|i| 0.2 + 0.6 * (i % w) as f32 / (w - 1) as f32)
            .collect();
        let dead = 8 * w + 8; // (8,8), normally ≈0.52; its 8 neighbours median ≈0.52
        pixels[dead] = 0.0;
        let flat = make_cfa(w, h, pixels, CfaType::Mono);

        let defect_map = DefectMap::default().detect_cold(&flat, &CancelToken::never());

        assert_eq!(defect_map.cold_count(), 1, "only the dead pixel is cold");
        assert_eq!(defect_map.hot_count(), 0, "a flat yields no hot pixels");
        assert!(is_cold(&defect_map, dead));
        assert!(
            !is_cold(&defect_map, 8 * w),
            "dim edge (0.2) is vignetting, not dead"
        );
        assert!(
            !is_cold(&defect_map, 8 * w + 15),
            "bright edge (0.8) is fine"
        );
    }

    /// `detect_hot` + `detect_cold` combine hot pixels (from the dark) and cold pixels (from the
    /// flat) into one map.
    #[test]
    fn detect_hot_and_cold_combine() {
        // Near-zero dark with one hot pixel; illuminated flat with one dead pixel.
        let mut dark_px = vec![0.001f32; 36];
        dark_px[0] = 0.5; // hot
        let dark = make_cfa(6, 6, dark_px, CfaType::Mono);

        let mut flat_px = vec![0.4f32; 36];
        flat_px[5] = 0.0; // dead
        let flat = make_cfa(6, 6, flat_px, CfaType::Mono);

        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .detect_cold(&flat, &CancelToken::never());

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
            dimensions: Some(Vec2us::new(2, 2)),
        };

        defect_map.correct(&mut image);
    }
}
