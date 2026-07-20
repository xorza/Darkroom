//! Defective-pixel detection and correction.
//!
//! **Hot** pixels (abnormally high dark current) come from the master dark after subtracting a
//! robust per-color tiled background, then applying a MAD threshold to the residual; **cold/dead**
//! pixels (abnormally low response) come from the master flat via a local-neighbourhood ratio test.
//! Both are corrected by replacing the pixel with the median of its same-color CFA neighbours.
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
//! 4. **Broad dark structure:**
//!    Per-color tile medians are bilinearly interpolated into a smooth dark-current model before
//!    thresholding. This prevents gradients and amp glow from becoming false point defects while
//!    preserving isolated pixels and same-color clusters as positive residuals.
//!
//! 5. **Adaptive sampling for large images:**
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
use crate::math::statistics::{MAD_TO_SIGMA, median_f32_mut};
use crate::stacking::combine::error::Error;
use common::BitBuffer2;
use common::CancelToken;
use common::Vec2us;
use imaginarium::Buffer2;

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
    /// Flat indices of hot pixels (above `median + kσ` in the background-subtracted dark),
    /// ascending.
    pub hot_indices: Vec<usize>,
    /// Flat indices of cold/dead pixels (below [`DEAD_PIXEL_FRACTION`] of their same-color
    /// local-neighbourhood median in the flat), ascending.
    pub cold_indices: Vec<usize>,
    /// Sensor dimensions the indices apply to — `None` until the first `detect_*` call records them.
    pub(crate) dimensions: Option<Vec2us>,
}

impl DefectMap {
    /// Resident RAM held by the map: its hot + cold flat-index lists.
    pub fn ram_bytes(&self) -> usize {
        (self.hot_indices.len() + self.cold_indices.len()) * std::mem::size_of::<usize>()
    }

    /// Detect **hot** pixels from a master dark — those whose residual above a smooth per-color
    /// dark background exceeds `median + sigma_threshold·σ` — and store them. Calls are chainable
    /// with `?`, in any order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if cancellation is requested before detection completes.
    pub fn detect_hot(
        mut self,
        dark: &CfaImage,
        sigma_threshold: f32,
        cancel: &CancelToken,
    ) -> Result<Self, Error> {
        // Clamp at the boundary rather than asserting: `sigma_threshold` may come from user config,
        // and a non-positive value (which would flag every pixel above the median) must not panic
        // the pipeline. Nothing below 1σ is a meaningful defect threshold.
        let sigma_threshold = sigma_threshold.max(MIN_SIGMA_THRESHOLD);
        self.set_dimensions(Vec2us::new(dark.data.width(), dark.data.height()));
        self.hot_indices = detect_hot_pixels(dark, sigma_threshold, cancel)?;
        Ok(self)
    }

    /// Detect **cold/dead** pixels from a master flat — those reading below [`DEAD_PIXEL_FRACTION`]
    /// of their same-color local-neighbourhood median — and store them. The local reference makes
    /// this robust to vignetting and dust, where a global cut cannot be.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if cancellation is requested before detection completes.
    pub fn detect_cold(mut self, flat: &CfaImage, cancel: &CancelToken) -> Result<Self, Error> {
        self.set_dimensions(Vec2us::new(flat.data.width(), flat.data.height()));
        self.cold_indices = detect_cold_pixels(flat, DEAD_PIXEL_FRACTION, cancel)?;
        Ok(self)
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
        let neighbors = SameColorMedian::new(Some(cfa_type));

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
            image.data[idx] = neighbors.at(&image.data, x, y, Some(&mask));
        }
    }
}

/// Maximum number of samples per color channel for median estimation.
const MAX_MEDIAN_SAMPLES: usize = 100_000;

/// Broad dark-current model tile size. Each tile has enough Bayer red/blue samples for a robust
/// median while remaining much smaller than normal sensor-scale gradients and amp glow.
const DARK_BACKGROUND_TILE_SIZE: usize = 64;

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

/// Flag hot pixels in a master dark: fit a robust broad per-color background, then threshold the
/// residual at `median + kσ` for its CFA color. Per-color keeps green (50% of Bayer data) from
/// masking red/blue defects.
fn detect_hot_pixels(
    image: &CfaImage,
    sigma_threshold: f32,
    cancel: &CancelToken,
) -> Result<Vec<usize>, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let data = &image.data;
    let width = data.width();
    let total = width * data.height();
    let cfa_type = image.metadata.cfa_type.as_ref();
    let background = DarkBackground::fit(data, cfa_type, cancel)?;
    let stats = compute_per_color_residual_stats(data, cfa_type, &background);

    // Indexed collect keeps the result ascending, preserving the map's binary-search invariant.
    // The broad model uses tile medians rather than same-color neighbour medians so a compact
    // same-color cluster remains an outlier instead of becoming its own local reference.
    let indices = (0..total)
        .into_par_iter()
        .filter(|&i| {
            if cancel.is_cancelled() {
                return false;
            }
            let color = cfa_color_at(cfa_type, i % width, i / width) as usize;
            let ColorStats { median, sigma } = stats[color];
            data[i] - background.at(i % width, i / width, color) > median + sigma_threshold * sigma
        })
        .collect();

    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(indices)
}

#[derive(Debug, Clone, Copy)]
struct InterpolationSpan {
    lower: usize,
    upper: usize,
    fraction: f32,
}

#[derive(Debug, Clone, Copy)]
struct DarkTile {
    values: [f32; 3],
}

/// Smooth per-CFA-color dark-current model sampled from robust tile medians.
#[derive(Debug)]
struct DarkBackground {
    tiles: Buffer2<DarkTile>,
    x_spans: Vec<InterpolationSpan>,
    y_spans: Vec<InterpolationSpan>,
}

impl DarkBackground {
    fn fit(
        data: &Buffer2<f32>,
        cfa_type: Option<&CfaType>,
        cancel: &CancelToken,
    ) -> Result<Self, Error> {
        let width = data.width();
        let height = data.height();
        assert!(
            width > 0 && height > 0,
            "dark background needs non-zero dimensions"
        );
        let tiles_x = width.div_ceil(DARK_BACKGROUND_TILE_SIZE);
        let tiles_y = height.div_ceil(DARK_BACKGROUND_TILE_SIZE);
        let num_colors = cfa_type.map_or(1, CfaType::num_colors);

        let mut tiles: Vec<DarkTile> = (0..tiles_x * tiles_y)
            .into_par_iter()
            .map(|index| {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }

                let tx = index % tiles_x;
                let ty = index / tiles_x;
                let x_start = tx * width / tiles_x;
                let x_end = (tx + 1) * width / tiles_x;
                let y_start = ty * height / tiles_y;
                let y_end = (ty + 1) * height / tiles_y;
                let mut samples: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::new());

                for y in y_start..y_end {
                    for x in x_start..x_end {
                        let color = cfa_color_at(cfa_type, x, y) as usize;
                        samples[color].push(data[y * width + x]);
                    }
                }

                let mut values = [f32::NAN; 3];
                for color in 0..num_colors {
                    if !samples[color].is_empty() {
                        values[color] = median_f32_mut(&mut samples[color]);
                    }
                }
                Ok(DarkTile { values })
            })
            .collect::<Result<_, Error>>()?;

        let missing: [bool; 3] = std::array::from_fn(|color| {
            color < num_colors && tiles.iter().any(|tile| tile.values[color].is_nan())
        });
        for (color, &is_missing) in missing.iter().enumerate().take(num_colors) {
            if !is_missing {
                continue;
            }
            let mut samples = collect_color_samples(data, cfa_type, color as u8);
            if samples.is_empty() {
                continue;
            }
            let fallback = median_f32_mut(&mut samples);
            for tile in &mut tiles {
                if tile.values[color].is_nan() {
                    tile.values[color] = fallback;
                }
            }
        }

        let centers_x = tile_centers(width, tiles_x);
        let centers_y = tile_centers(height, tiles_y);
        Ok(Self {
            tiles: Buffer2::new(tiles_x, tiles_y, tiles),
            x_spans: interpolation_spans(width, &centers_x),
            y_spans: interpolation_spans(height, &centers_y),
        })
    }

    #[inline]
    fn at(&self, x: usize, y: usize, color: usize) -> f32 {
        let xs = self.x_spans[x];
        let ys = self.y_spans[y];
        let top = lerp(
            self.tiles[(xs.lower, ys.lower)].values[color],
            self.tiles[(xs.upper, ys.lower)].values[color],
            xs.fraction,
        );
        let bottom = lerp(
            self.tiles[(xs.lower, ys.upper)].values[color],
            self.tiles[(xs.upper, ys.upper)].values[color],
            xs.fraction,
        );
        lerp(top, bottom, ys.fraction)
    }
}

fn tile_centers(length: usize, tile_count: usize) -> Vec<f32> {
    (0..tile_count)
        .map(|tile| {
            let start = tile * length / tile_count;
            let end = (tile + 1) * length / tile_count;
            (start + end - 1) as f32 * 0.5
        })
        .collect()
}

fn interpolation_spans(length: usize, centers: &[f32]) -> Vec<InterpolationSpan> {
    if centers.len() == 1 {
        return vec![
            InterpolationSpan {
                lower: 0,
                upper: 0,
                fraction: 0.0,
            };
            length
        ];
    }

    (0..length)
        .map(|position| {
            let position = position as f32;
            let upper = centers
                .partition_point(|&center| center <= position)
                .clamp(1, centers.len() - 1);
            let lower = upper - 1;
            InterpolationSpan {
                lower,
                upper,
                fraction: (position - centers[lower]) / (centers[upper] - centers[lower]),
            }
        })
        .collect()
}

#[inline]
fn lerp(start: f32, end: f32, fraction: f32) -> f32 {
    start + fraction * (end - start)
}

/// Flag cold/dead pixels in a master flat: those reading below `dead_fraction` of the median of
/// their same-color local neighbours. The local reference tracks vignetting (so a global cut's
/// negative-threshold failure can't happen) and ignores dust shadows; only near-zero pixels pass.
/// The neighbour scan runs on every pixel in parallel — one-time work, off the hot path.
fn detect_cold_pixels(
    image: &CfaImage,
    dead_fraction: f32,
    cancel: &CancelToken,
) -> Result<Vec<usize>, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let data = &image.data;
    let width = data.width();
    let total = width * data.height();
    let neighbors = SameColorMedian::new(image.metadata.cfa_type.as_ref());

    let indices = (0..total)
        .into_par_iter()
        .filter(|&i| {
            if cancel.is_cancelled() {
                return false;
            }
            let local = neighbors.at(data, i % width, i / width, None);
            data[i] < dead_fraction * local
        })
        .collect();

    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(indices)
}

/// Per-CFA-color robust residual statistics used to threshold hot pixels.
#[derive(Debug, Clone, Copy)]
struct ColorStats {
    /// Median residual for the color (the hot-detection center).
    median: f32,
    /// Robust σ (`MAD · 1.4826`, floored). A color with no samples gets `∞` so it never flags.
    sigma: f32,
}

/// Per-CFA-color robust background-subtracted stats, indexed by color (0=R/mono, 1=G, 2=B).
///
/// `sigma` is `MAD · 1.4826` floored only at an absolute `5e-4` (~33 ADU/16-bit) so a clean
/// near-uniform master can't flag its own noise tail. (A former relative floor `median · 0.1` was
/// dropped: it capped sensitivity at ~1.5× the median, hiding warm pixels at 1.1–1.4× median, and
/// has no precedent in PixInsight/Siril/APP/ccdmask. The absolute floor alone is the standard
/// guard.) A color with no samples gets `sigma = ∞` so it never flags.
fn compute_per_color_residual_stats(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    background: &DarkBackground,
) -> ArrayVec<ColorStats, 3> {
    let num_colors = cfa_type.map_or(1, |c| c.num_colors());
    let mut stats = ArrayVec::new();

    for color in 0..num_colors as u8 {
        let mut samples = collect_color_residual_samples(data, cfa_type, color, background);

        if samples.is_empty() {
            stats.push(ColorStats {
                median: 0.0,
                sigma: f32::INFINITY,
            });
            continue;
        }

        let median = median_f32_mut(&mut samples);
        for v in samples.iter_mut() {
            *v = (*v - median).abs();
        }
        let mad = median_f32_mut(&mut samples);
        let sigma = (mad * MAD_TO_SIGMA).max(5e-4);

        tracing::debug!(
            "Defect residual stats color={color}: median={median:.6}, MAD={mad:.6}, sigma={sigma:.6}"
        );
        stats.push(ColorStats { median, sigma });
    }

    stats
}

fn collect_color_residual_samples(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    target_color: u8,
    background: &DarkBackground,
) -> Vec<f32> {
    let width = data.width();
    collect_color_sample_indices(data, cfa_type, target_color)
        .map(|index| {
            data[index] - background.at(index % width, index / width, target_color as usize)
        })
        .collect()
}

/// Collect pixel samples for a specific CFA color channel.
///
/// Uses uniform sampling when the channel has more than `MAX_MEDIAN_SAMPLES * 2` pixels.
fn collect_color_samples(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    target_color: u8,
) -> Vec<f32> {
    collect_color_sample_indices(data, cfa_type, target_color)
        .map(|index| data[index])
        .collect()
}

fn collect_color_sample_indices<'a>(
    data: &'a Buffer2<f32>,
    cfa_type: Option<&'a CfaType>,
    target_color: u8,
) -> impl Iterator<Item = usize> + 'a {
    let width = data.width();
    let height = data.height();
    let total = width * height;
    let keep_stride = if total > MAX_MEDIAN_SAMPLES * 2 {
        total.div_ceil(MAX_MEDIAN_SAMPLES)
    } else {
        1
    };
    let mut seen = 0usize;
    (0..total).filter(move |&index| {
        let x = index % width;
        let y = index / width;
        if cfa_color_at(cfa_type, x, y) != target_color {
            return false;
        }
        let keep = seen.is_multiple_of(keep_stride);
        seen += 1;
        keep
    })
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

/// Same-color CFA neighbour median strategy, built once per master so the per-pixel scan stays
/// cheap. The X-Trans variant precomputes its neighbour geometry up front (see [`XTransOffsets`]);
/// Mono and Bayer carry no state because their offsets are fixed.
#[derive(Debug)]
enum SameColorMedian {
    /// Mono: 8-connected neighbours.
    Mono,
    /// Bayer: same-color neighbours at stride 2 (true for every 2×2 Bayer phase).
    Bayer,
    /// X-Trans: same-color offsets precomputed per 6×6 phase. Boxed — the 36-phase table dwarfs the
    /// other (zero-size) variants, and the strategy is built once per master, not per pixel.
    XTrans(Box<XTransOffsets>),
}

impl SameColorMedian {
    /// `None` (no CFA metadata) is treated as Mono, matching [`cfa_color_at`].
    fn new(cfa: Option<&CfaType>) -> Self {
        match cfa {
            None | Some(CfaType::Mono) => Self::Mono,
            Some(CfaType::Bayer(_)) => Self::Bayer,
            Some(CfaType::XTrans(pattern)) => Self::XTrans(Box::new(XTransOffsets::new(pattern))),
        }
    }

    /// Median of `(x, y)`'s same-color neighbours, skipping any flagged in `defect_mask` so a defect
    /// is never repaired from another defect.
    fn at(
        &self,
        pixels: &Buffer2<f32>,
        x: usize,
        y: usize,
        defect_mask: Option<&BitBuffer2>,
    ) -> f32 {
        match self {
            Self::Mono => median_of_neighbors_raw(pixels, x, y, defect_mask),
            Self::Bayer => bayer_same_color_median(pixels, x, y, defect_mask),
            Self::XTrans(offsets) => offsets.median(pixels, x, y, defect_mask),
        }
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

/// Search radius (in pixels) for X-Trans same-color neighbours — one full pattern period.
const XTRANS_RADIUS: i32 = 6;

/// Same-color neighbours used for the X-Trans median: ≈4 per cardinal/diagonal direction in the
/// 6×6 pattern — enough for a robust median without directional bias.
const XTRANS_NEIGHBORS: usize = 24;

/// Precomputed X-Trans same-color neighbour offsets, indexed by the pixel's 6×6 phase.
///
/// The X-Trans pattern is periodic with period 6, so for a given phase `(x % 6, y % 6)` the set of
/// same-color neighbours within the search window — and their Manhattan distances — is fixed. The
/// old path recomputed this on every pixel: a 13×13 `color_at` sweep plus a per-pixel distance
/// sort, which dominated the cold-pixel scan of a full X-Trans master. Precomputing it once turns
/// the per-pixel work into a bounded gather + median over the nearest valid neighbours.
#[derive(Debug)]
struct XTransOffsets {
    /// `per_phase[(y % 6) * 6 + (x % 6)]` = same-color `(dx, dy)` offsets, nearest-first by
    /// Manhattan distance (ties broken by scan order: `dy` then `dx`).
    per_phase: [Vec<(i32, i32)>; 36],
}

impl XTransOffsets {
    fn new(pattern: &[[u8; 6]; 6]) -> Self {
        let per_phase = std::array::from_fn(|phase| {
            let px = (phase % 6) as i32;
            let py = (phase / 6) as i32;
            let my_color = pattern[py as usize][px as usize];

            let mut candidates: Vec<(i32, (i32, i32))> = Vec::new();
            for dy in -XTRANS_RADIUS..=XTRANS_RADIUS {
                for dx in -XTRANS_RADIUS..=XTRANS_RADIUS {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    // The pattern is globally periodic, so a neighbour's color depends only on its
                    // phase; `rem_euclid` keeps that correct for negative offsets.
                    let cy = (py + dy).rem_euclid(6) as usize;
                    let cx = (px + dx).rem_euclid(6) as usize;
                    if pattern[cy][cx] == my_color {
                        candidates.push((dx.abs() + dy.abs(), (dx, dy)));
                    }
                }
            }
            // Stable sort: equal-distance neighbours keep scan order.
            candidates.sort_by_key(|&(dist, _)| dist);
            candidates.into_iter().map(|(_, off)| off).collect()
        });
        Self { per_phase }
    }

    /// Median of the nearest valid same-color neighbours of `(x, y)`. Walks the precomputed
    /// nearest-first offsets, skipping out-of-bounds and `defect_mask`ed positions, and stops once
    /// [`XTRANS_NEIGHBORS`] valid samples are gathered — equivalent to "closest-N valid neighbours".
    fn median(
        &self,
        pixels: &Buffer2<f32>,
        x: usize,
        y: usize,
        defect_mask: Option<&BitBuffer2>,
    ) -> f32 {
        let width = pixels.width() as i32;
        let height = pixels.height() as i32;
        let mut buf = [0.0f32; XTRANS_NEIGHBORS];
        let mut count = 0;
        for &(dx, dy) in &self.per_phase[(y % 6) * 6 + (x % 6)] {
            if count == XTRANS_NEIGHBORS {
                break;
            }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width || ny >= height {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if defect_mask.is_some_and(|m| m.get_xy(nx, ny)) {
                continue;
            }
            buf[count] = *pixels.get(nx, ny);
            count += 1;
        }
        if count == 0 {
            return *pixels.get(x, y);
        }
        median_f32_mut(&mut buf[..count])
    }
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
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::stacking::calibration_masters::defect_map::*;
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
    use crate::stacking::calibration_masters::defect_map::*;
    use crate::{io::raw::demosaic::bayer::CfaPattern, testing::make_cfa};

    fn is_hot(defect_map: &DefectMap, pixel_idx: usize) -> bool {
        defect_map.hot_indices.binary_search(&pixel_idx).is_ok()
    }

    fn is_cold(defect_map: &DefectMap, pixel_idx: usize) -> bool {
        defect_map.cold_indices.binary_search(&pixel_idx).is_ok()
    }

    #[test]
    fn cancelled_detection_returns_error() {
        let image = make_cfa(4, 4, vec![0.5; 16], CfaType::Mono);
        let cancel = CancelToken::new();
        cancel.cancel();

        assert!(matches!(
            DefectMap::default().detect_hot(&image, 5.0, &cancel),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            DefectMap::default().detect_cold(&image, &cancel),
            Err(Error::Cancelled)
        ));
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
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();
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
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();
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
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();

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
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();

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
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 3.0, &CancelToken::never())
            .unwrap();

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
    fn dark_background_reconstructs_affine_mono_signal_through_image_edges() {
        let (width, height) = (192usize, 128usize);
        let pixels: Vec<f32> = (0..width * height)
            .map(|index| {
                let x = (index % width) as f32;
                let y = (index / width) as f32;
                0.02 + 0.0001 * x + 0.0002 * y
            })
            .collect();
        let data = Buffer2::new(width, height, pixels);
        let background =
            DarkBackground::fit(&data, Some(&CfaType::Mono), &CancelToken::never()).unwrap();

        for y in 0..height {
            for x in 0..width {
                let expected = 0.02 + 0.0001 * x as f32 + 0.0002 * y as f32;
                assert!(
                    (background.at(x, y, 0) - expected).abs() < 2e-7,
                    "affine background mismatch at ({x}, {y}): expected {expected}, got {}",
                    background.at(x, y, 0)
                );
            }
        }
    }

    #[test]
    fn hot_detection_removes_per_color_gradient_and_amp_glow_but_keeps_clusters() {
        let (width, height) = (384usize, 256usize);
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        let isolated = [(17usize, 23usize), (201, 48), (312, 190)];
        let mut cluster = Vec::new();
        for y in (124..=132).step_by(2) {
            for x in (156..=164).step_by(2) {
                assert_eq!(cfa.color_at(x, y), 0);
                cluster.push((x, y));
            }
        }

        let mut pixels = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let color = cfa.color_at(x, y) as usize;
                let x_unit = x as f32 / (width - 1) as f32;
                let y_unit = y as f32 / (height - 1) as f32;
                let baseline = [0.01, 0.02, 0.03][color];
                let gradient = [0.012, 0.018, 0.024][color] * x_unit + 0.01 * y_unit;
                let amp_glow = [0.05, 0.07, 0.09][color] * x_unit * x_unit * (0.5 + 0.5 * y_unit);
                let noise = ((x * 37 + y * 19) % 17) as f32 * 0.00003 - 0.00024;
                pixels[y * width + x] = baseline + gradient + amp_glow + noise;
            }
        }

        let mut expected: Vec<usize> = isolated
            .iter()
            .chain(&cluster)
            .map(|&(x, y)| y * width + x)
            .collect();
        expected.sort_unstable();
        for &index in &expected {
            pixels[index] += 0.08;
        }

        let dark = make_cfa(width, height, pixels, cfa);
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();

        assert_eq!(
            defect_map.hot_indices, expected,
            "smooth per-color structure must not become defects, while every injected point and \
             same-color cluster member must remain detectable"
        );
    }

    #[test]
    fn test_cfa_no_defective_pixels() {
        let pixels = vec![100.0; 36];
        let dark = make_cfa(6, 6, pixels, CfaType::Mono);
        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();
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

        let defect_map = DefectMap::default()
            .detect_hot(&dark, 5.0, &CancelToken::never())
            .unwrap();

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

        let defect_map = DefectMap::default()
            .detect_cold(&flat, &CancelToken::never())
            .unwrap();

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
        let defect_map = DefectMap::default()
            .detect_cold(&flat, &CancelToken::never())
            .unwrap();
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
        for y in 6..10 {
            for x in 2..6 {
                pixels[y * w + x] *= 0.65;
            }
        }
        let dead = 8 * w + 8; // (8,8), normally ≈0.52; its 8 neighbours median ≈0.52
        pixels[dead] = 0.0;
        let flat = make_cfa(w, h, pixels, CfaType::Mono);

        let defect_map = DefectMap::default()
            .detect_cold(&flat, &CancelToken::never())
            .unwrap();

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
        for y in 6..10 {
            for x in 2..6 {
                assert!(
                    !is_cold(&defect_map, y * w + x),
                    "dust shadow ({x}, {y}) is attenuated, not dead"
                );
            }
        }
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
            .unwrap()
            .detect_cold(&flat, &CancelToken::never())
            .unwrap();

        assert_eq!(defect_map.hot_count(), 1);
        assert_eq!(defect_map.cold_count(), 1);
        assert_eq!(defect_map.count(), 2);
        assert!(is_hot(&defect_map, 0));
        assert!(is_cold(&defect_map, 5));
    }

    /// A representative non-trivial X-Trans pattern (R=0, G=1, B=2) reused by the X-Trans tests.
    const XTRANS_PATTERN: [[u8; 6]; 6] = [
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ];

    /// Reference X-Trans same-color median: collect every in-bounds, unmasked same-color neighbour
    /// in the radius-6 window, take the closest `XTRANS_NEIGHBORS` by Manhattan distance (ties in
    /// scan order), median them. The precomputed [`XTransOffsets`] must reproduce this exactly.
    fn brute_force_xtrans_median(
        pixels: &Buffer2<f32>,
        x: usize,
        y: usize,
        pattern: &CfaType,
    ) -> f32 {
        let (w, h) = (pixels.width() as i32, pixels.height() as i32);
        let my_color = pattern.color_at(x, y);
        let mut cands: Vec<(i32, f32)> = Vec::new();
        for dy in -XTRANS_RADIUS..=XTRANS_RADIUS {
            for dx in -XTRANS_RADIUS..=XTRANS_RADIUS {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                if pattern.color_at(nx as usize, ny as usize) == my_color {
                    cands.push((dx.abs() + dy.abs(), *pixels.get(nx as usize, ny as usize)));
                }
            }
        }
        cands.sort_by_key(|&(dist, _)| dist);
        let n = cands.len().min(XTRANS_NEIGHBORS);
        let mut vals: Vec<f32> = cands[..n].iter().map(|&(_, v)| v).collect();
        median_f32_mut(&mut vals)
    }

    /// The precomputed X-Trans offsets must reproduce the brute-force closest-N same-color median at
    /// every pixel — including borders (fewer neighbours) and interior (the N-cutoff is exercised).
    #[test]
    fn xtrans_offsets_match_brute_force() {
        let pattern = CfaType::XTrans(XTRANS_PATTERN);
        let (w, h) = (29usize, 23usize); // not a multiple of 6, so all 36 phases hit the borders
        // Deterministic, well-spread values so medians are sensitive to which neighbours are chosen.
        let px: Vec<f32> = (0..w * h)
            .map(|i| ((i.wrapping_mul(2_654_435_761) >> 8) % 1000) as f32 / 1000.0)
            .collect();
        let pixels = Buffer2::new(w, h, px);
        let offsets = XTransOffsets::new(&XTRANS_PATTERN);

        for y in 0..h {
            for x in 0..w {
                let got = offsets.median(&pixels, x, y, None);
                let want = brute_force_xtrans_median(&pixels, x, y, &pattern);
                assert_eq!(
                    got, want,
                    "X-Trans median mismatch at ({x},{y}): precomputed {got} vs brute-force {want}"
                );
            }
        }
    }

    /// X-Trans same-color selection: with each color held at a distinct constant, an interior
    /// pixel's same-color median is exactly its own color's value (a wrong-color pick would mix them).
    #[test]
    fn xtrans_median_selects_same_color() {
        let pattern = CfaType::XTrans(XTRANS_PATTERN);
        let (w, h) = (24usize, 24usize);
        let color_val = |c: u8| 0.1 * (c + 1) as f32; // R→0.1, G→0.2, B→0.3
        let px: Vec<f32> = (0..w * h)
            .map(|i| color_val(pattern.color_at(i % w, i / w)))
            .collect();
        let pixels = Buffer2::new(w, h, px);
        let neighbors = SameColorMedian::new(Some(&pattern));

        // Interior pixels (≥6 from every border) of each color — all 24 nearest same-color in-bounds.
        for &(x, y) in &[(13usize, 12usize), (12, 12), (14, 13)] {
            let c = pattern.color_at(x, y);
            let got = neighbors.at(&pixels, x, y, None);
            assert!(
                (got - color_val(c)).abs() < f32::EPSILON,
                "({x},{y}) color {c}: expected {} got {got}",
                color_val(c)
            );
        }
    }

    /// X-Trans cold detection: a dead pixel reads below half its same-color neighbourhood and is the
    /// only one flagged; a normal pixel (value == its neighbourhood median) is not.
    #[test]
    fn xtrans_cold_pixel_detected() {
        let pattern = CfaType::XTrans(XTRANS_PATTERN);
        let (w, h) = (24usize, 24usize);
        let color_val = |c: u8| 0.1 * (c + 1) as f32;
        let mut px: Vec<f32> = (0..w * h)
            .map(|i| color_val(pattern.color_at(i % w, i / w)))
            .collect();
        let dead = 12 * w + 12; // interior G pixel: 0.0 < 0.5 · 0.2 neighbourhood median
        assert_eq!(pattern.color_at(12, 12), 1, "(12,12) is green");
        px[dead] = 0.0;
        let flat = make_cfa(w, h, px, pattern);

        let defect_map = DefectMap::default()
            .detect_cold(&flat, &CancelToken::never())
            .unwrap();

        assert_eq!(defect_map.cold_count(), 1, "only the dead pixel is cold");
        assert_eq!(defect_map.hot_count(), 0, "detect_cold sets no hot pixels");
        assert!(is_cold(&defect_map, dead));
        // A normal R neighbour: one dead neighbour can't drag its 24-sample median below half.
        assert!(!is_cold(&defect_map, 12 * w + 13));
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
