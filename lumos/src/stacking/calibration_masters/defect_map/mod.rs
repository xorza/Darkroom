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

pub(crate) mod dark_background;
mod same_color;
mod sampling;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::size2us::Size2us;
use crate::math::statistics::{MAD_TO_SIGMA, median_f32_mut};
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::defect_map::dark_background::DarkBackground;
use crate::stacking::calibration_masters::defect_map::same_color::SameColorMedian;
use crate::stacking::calibration_masters::defect_map::sampling::collect_color_residual_samples;
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
    /// Flat indices of cold/dead pixels (below `DEAD_PIXEL_FRACTION` of their same-color
    /// local-neighbourhood median in the flat), ascending.
    pub cold_indices: Vec<usize>,
    /// Sensor dimensions the indices apply to — `None` until the first `detect_*` call records them.
    pub(super) dimensions: Option<Size2us>,
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
        self.set_dimensions(Size2us::new(dark.data.width(), dark.data.height()));
        self.hot_indices = detect_hot_pixels(dark, sigma_threshold, cancel)?;
        Ok(self)
    }

    /// Detect **cold/dead** pixels from a master flat — those reading below `DEAD_PIXEL_FRACTION`
    /// of their same-color local-neighbourhood median — and store them. The local reference makes
    /// this robust to vignetting and dust, where a global cut cannot be.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if cancellation is requested before detection completes.
    pub fn detect_cold(mut self, flat: &CfaImage, cancel: &CancelToken) -> Result<Self, Error> {
        self.set_dimensions(Size2us::new(flat.data.width(), flat.data.height()));
        self.cold_indices = detect_cold_pixels(flat, DEAD_PIXEL_FRACTION, cancel)?;
        Ok(self)
    }

    /// Record the master dimensions on first detection, or assert they match on later calls: every
    /// master feeding one map corrects the same sensor, so all must share dimensions.
    fn set_dimensions(&mut self, dims: Size2us) {
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
        self.dimensions.map_or(0.0, |size| {
            100.0 * self.count() as f32 / size.pixel_count() as f32
        })
    }

    /// Correct defective pixels on raw CFA data by replacing with median of
    /// same-color CFA neighbors.
    pub fn correct(&self, image: &mut CfaImage) {
        let size = self
            .dimensions
            .expect("defect map has no dimensions; detect a master first");
        assert!(
            Size2us::new(image.data.width(), image.data.height()) == size,
            "CfaImage dimensions {}x{} don't match defect pixel map {}x{}",
            image.data.width(),
            image.data.height(),
            size.width,
            size.height
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
        let mut mask = BitBuffer2::new_default(Size2us::new(size.width, size.height));
        for &idx in self.hot_indices.iter().chain(&self.cold_indices) {
            mask.set(idx, true);
        }

        for &idx in self.hot_indices.iter().chain(&self.cold_indices) {
            image.data[idx] = neighbors.at(&image.data, size.point_of(idx), Some(&mask));
        }
    }
}

/// Maximum number of samples per color channel for median estimation.
pub(super) const MAX_MEDIAN_SAMPLES: usize = 100_000;

/// Broad dark-current model tile size. Each tile has enough Bayer red/blue samples for a robust
/// median while remaining much smaller than normal sensor-scale gradients and amp glow.
pub(super) const DARK_BACKGROUND_TILE_SIZE: usize = 64;

/// Convert the 99th percentile of `|N(0, σ)|` back to σ.
const ABSOLUTE_RESIDUAL_P99_TO_SIGMA: f32 = 0.388_224_48;
// Five expected tail samples keep one sparse defect from defining the scale on tiny images.
const MIN_TAIL_SCALE_SAMPLES: usize = 500;

/// Get CFA color index at (x, y). Returns 0 for Mono (None CFA type).
pub(super) fn cfa_color_at(cfa_type: Option<&CfaType>, pos: Vec2us) -> u8 {
    match cfa_type {
        Some(cfa) => cfa.color_at(pos),
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
    let size = Size2us::new(data.width(), data.height());
    let total = size.pixel_count();
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
            let point = size.point_of(i);
            let color = cfa_color_at(cfa_type, point) as usize;
            let ColorStats { median, sigma } = stats[color];
            data[i] - background.at(point, color) > median + sigma_threshold * sigma
        })
        .collect();

    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(indices)
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
    let size = Size2us::new(data.width(), data.height());
    let total = size.pixel_count();
    let neighbors = SameColorMedian::new(image.metadata.cfa_type.as_ref());

    let indices = (0..total)
        .into_par_iter()
        .filter(|&i| {
            if cancel.is_cancelled() {
                return false;
            }
            let local = neighbors.at(data, size.point_of(i), None);
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

/// Per-class defect counts, used only by tests to assert detection behavior.
#[cfg(test)]
mod internals {
    use super::*;

    impl DefectMap {
        /// Number of hot pixels detected.
        pub(crate) fn hot_count(&self) -> usize {
            self.hot_indices.len()
        }

        /// Number of cold/dead pixels detected.
        pub(crate) fn cold_count(&self) -> usize {
            self.cold_indices.len()
        }
    }
}

#[cfg(all(test, feature = "internals"))]
mod bench;

#[cfg(test)]
mod tests;
