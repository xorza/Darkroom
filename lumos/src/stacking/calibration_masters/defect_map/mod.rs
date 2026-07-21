//! Defective-pixel detection and correction.
//!
//! **Hot** pixels (abnormally high dark current) come from the master dark after subtracting a
//! robust per-color tiled background, then thresholding against a robust residual scale;
//! **cold/dead** pixels (abnormally low response) come from the master flat via a
//! local-neighbourhood ratio test. Both are corrected by replacing the pixel with the median of
//! its same-color CFA neighbours.
//!
//! # Hot pixels (from the dark)
//!
//! Uses robust per-color σ estimation led by **Median Absolute Deviation (MAD)**:
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
//!    Exact median computation is slow on full-resolution sensors. Each color receives up to 100K
//!    samples, distributed across its CFA phases and the full sensor rows and columns.
//!
//! 6. **Quantization-aware zero-MAD handling:**
//!    A perfectly stable master can have zero MAD because its samples occupy one quantization
//!    level. The σ floor follows the RAW ADC step propagated through master-frame stacking, with
//!    floating-point resolution as the fallback when source quantization is unknown.
//!
//! # Cold/dead pixels (from the flat)
//!
//! A *global* threshold cannot find dead pixels in a real flat: vignetting spreads the per-color
//! values so wide that `median − kσ` falls below zero, so nothing is ever flagged. Instead a
//! pixel is dead when it reads below [`DEAD_PIXEL_FRACTION`] of the median of its *same-color
//! local neighbours* — a reference that tracks vignetting (smooth, locally flat) and ignores dust
//! shadows (which dim by far less than half), so only genuinely near-zero pixels are caught.

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::statistics::{MAD_TO_SIGMA, median_f32_mut};
use crate::math::vec2us::Vec2us;
use crate::stacking::combine::error::Error;
use common::CancelToken;
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

/// Convert the 99th percentile of `|N(0, σ)|` back to σ.
const ABSOLUTE_RESIDUAL_P99_TO_SIGMA: f32 = 0.388_224_48;
// Five expected tail samples keep one sparse defect from defining the scale on tiny images.
const MIN_TAIL_SCALE_SAMPLES: usize = 500;

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
    let sigma_floor = residual_sigma_floor(image);
    let stats = compute_per_color_residual_stats(data, cfa_type, &background, sigma_floor);

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

fn residual_sigma_floor(image: &CfaImage) -> f32 {
    if let Some(sigma) = image
        .quantization_sigma
        .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
    {
        return sigma;
    }
    image
        .data
        .par_iter()
        .map(|value| value.abs())
        .reduce(|| 0.0, f32::max)
        * f32::EPSILON
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
    /// Robust σ from MAD and the upper residual bulk, resolution-floored. No samples gives `∞`.
    sigma: f32,
}

#[derive(Debug, Clone, Copy)]
struct CfaSamplePhase {
    x_offset: usize,
    y_offset: usize,
    columns: usize,
    rows: usize,
    population: usize,
    sample_count: usize,
}

/// Per-CFA-color robust background-subtracted stats, indexed by color (0=R/mono, 1=G, 2=B).
///
/// `sigma` takes the larger of MAD and the Gaussian-calibrated 99th absolute residual percentile.
/// The latter keeps broad model error and column structure out of the defect tail while remaining
/// insensitive to a sparse (<1%) defect population. The result is floored at the master image's
/// quantization/numeric resolution so a zero-MAD plateau does not turn every representable
/// deviation into a defect. A color with no samples gets `sigma = ∞` so it never flags.
fn compute_per_color_residual_stats(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    background: &DarkBackground,
    sigma_floor: f32,
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
        let tail_sigma = if samples.len() >= MIN_TAIL_SCALE_SAMPLES {
            let p99_index = (samples.len() - 1) * 99 / 100;
            let (_, p99, _) = samples.select_nth_unstable_by(p99_index, f32::total_cmp);
            *p99 * ABSOLUTE_RESIDUAL_P99_TO_SIGMA
        } else {
            0.0
        };
        let sigma = (mad * MAD_TO_SIGMA).max(tail_sigma).max(sigma_floor);

        tracing::debug!(
            "Defect residual stats color={color}: median={median:.6}, MAD={mad:.6}, \
             tail_sigma={tail_sigma:.6}, floor={sigma_floor:.6}, sigma={sigma:.6}"
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
    collect_color_sample_indices(width, data.height(), cfa_type, target_color)
        .into_iter()
        .map(|index| {
            data[index] - background.at(index % width, index / width, target_color as usize)
        })
        .collect()
}

/// Collect pixel samples for a specific CFA color channel.
///
/// Large channels are stratified across CFA phases, rows, and columns.
fn collect_color_samples(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    target_color: u8,
) -> Vec<f32> {
    collect_color_sample_indices(data.width(), data.height(), cfa_type, target_color)
        .into_iter()
        .map(|index| data[index])
        .collect()
}

fn collect_color_sample_indices(
    width: usize,
    height: usize,
    cfa_type: Option<&CfaType>,
    target_color: u8,
) -> Vec<usize> {
    assert!(
        width > 0 && height > 0,
        "color sampling needs non-zero dimensions"
    );

    let period = match cfa_type {
        None | Some(CfaType::Mono) => 1,
        Some(CfaType::Bayer(_)) => 2,
        Some(CfaType::XTrans(_)) => 6,
    };

    let mut phases = ArrayVec::<CfaSamplePhase, 36>::new();
    for y_offset in 0..period.min(height) {
        for x_offset in 0..period.min(width) {
            if cfa_color_at(cfa_type, x_offset, y_offset) != target_color {
                continue;
            }
            let columns = (width - 1 - x_offset) / period + 1;
            let rows = (height - 1 - y_offset) / period + 1;
            phases.push(CfaSamplePhase {
                x_offset,
                y_offset,
                columns,
                rows,
                population: columns * rows,
                sample_count: 0,
            });
        }
    }

    let population: usize = phases.iter().map(|phase| phase.population).sum();
    if population == 0 {
        return Vec::new();
    }
    let target_sample_count = population.min(MAX_MEDIAN_SAMPLES);
    let mut cumulative_population = 0;
    let mut allocated = 0;
    for phase in &mut phases {
        cumulative_population += phase.population;
        let next_allocated =
            scaled_partition(cumulative_population, population, target_sample_count);
        phase.sample_count = next_allocated - allocated;
        allocated = next_allocated;
    }

    let mut indices = Vec::with_capacity(target_sample_count);
    for phase in phases {
        if phase.sample_count == phase.population {
            for row in 0..phase.rows {
                let y = phase.y_offset + row * period;
                for column in 0..phase.columns {
                    let x = phase.x_offset + column * period;
                    indices.push(y * width + x);
                }
            }
            continue;
        }

        let sampled_rows = phase.rows.min(phase.sample_count);
        let phase_rotation = phase.y_offset * period + phase.x_offset;
        for sample_row in 0..sampled_rows {
            let row = stratified_center(sample_row, sampled_rows, phase.rows);
            let y = phase.y_offset + row * period;
            let row_sample_start = scaled_partition(sample_row, sampled_rows, phase.sample_count);
            let row_sample_end = scaled_partition(sample_row + 1, sampled_rows, phase.sample_count);
            let row_sample_count = row_sample_end - row_sample_start;
            let rotation = (sample_row + phase_rotation) % phase.columns;

            for sample_column in 0..row_sample_count {
                let column = (stratified_center(sample_column, row_sample_count, phase.columns)
                    + rotation)
                    % phase.columns;
                let x = phase.x_offset + column * period;
                indices.push(y * width + x);
            }
        }
    }

    indices.sort_unstable();
    debug_assert_eq!(indices.len(), target_sample_count);
    debug_assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
    indices
}

fn scaled_partition(part: usize, part_count: usize, length: usize) -> usize {
    (part as u128 * length as u128 / part_count as u128) as usize
}

fn stratified_center(part: usize, part_count: usize, length: usize) -> usize {
    ((2 * part as u128 + 1) * length as u128 / (2 * part_count as u128)) as usize
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
mod bench;

#[cfg(test)]
mod tests;
