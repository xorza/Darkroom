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

pub(crate) mod sampling;

use crate::background_mesh::colour_mesh::ColourMesh;
use crate::background_mesh::workspace::MeshWorkspace;
use crate::bit_buffer2::BitBuffer2;
use crate::calibration_masters::defect_map::sampling::collect_color_residual_samples;
use crate::calibration_masters::error::CalibrationError;
use crate::io::image::cfa::cfa_lattice::{CfaLattice, Gathered};
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::size2us::Size2us;
use crate::math::statistics::{MedianMad, mad_to_sigma};
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
#[derive(Debug, Clone)]
pub struct DefectMap {
    hot_indices: Vec<usize>,
    cold_indices: Vec<usize>,
    /// Every defect, hot or cold, once: what a repair keeps out of its neighbours, built when
    /// the lists change rather than for every light corrected.
    mask: BitBuffer2,
    /// Pixels set in `mask` — a pixel both hot and dead counts once.
    count: usize,
    dimensions: Size2us,
}

impl DefectMap {
    /// An empty map for a sensor of `dimensions`, ready for [`Self::detect_hot`] and
    /// [`Self::detect_cold`].
    pub fn new(dimensions: Size2us) -> Self {
        Self {
            hot_indices: Vec::new(),
            cold_indices: Vec::new(),
            mask: BitBuffer2::new_default(dimensions),
            count: 0,
            dimensions,
        }
    }

    /// A map with these defect lists, or `None` when an index lies outside `dimensions`.
    pub(crate) fn from_indices(
        dimensions: Size2us,
        hot_indices: Vec<usize>,
        cold_indices: Vec<usize>,
    ) -> Option<Self> {
        let pixel_count = dimensions.pixel_count();
        if hot_indices
            .iter()
            .chain(&cold_indices)
            .any(|&index| index >= pixel_count)
        {
            return None;
        }
        let mut map = Self::new(dimensions);
        map.hot_indices = hot_indices;
        map.cold_indices = cold_indices;
        map.rebuild_mask();
        Some(map)
    }

    /// The sensor extent the indices apply to.
    pub const fn dimensions(&self) -> Size2us {
        self.dimensions
    }

    /// Flat indices of hot pixels — above `median + kσ` in the background-subtracted dark.
    pub fn hot_indices(&self) -> &[usize] {
        &self.hot_indices
    }

    /// Flat indices of cold/dead pixels — below `DEAD_PIXEL_FRACTION` of their same-color
    /// local-neighbourhood median in the flat.
    pub fn cold_indices(&self) -> &[usize] {
        &self.cold_indices
    }

    /// Resident RAM held by the map: its hot and cold index lists and its mask.
    pub const fn ram_bytes(&self) -> usize {
        (self.hot_indices.len() + self.cold_indices.len()) * size_of::<usize>()
            + self.mask.words.len() * size_of::<u64>()
    }

    /// Detect **hot** pixels from a master dark — those whose residual above a smooth per-color
    /// dark background exceeds `median + sigma_threshold·σ` — and store them. Calls are chainable
    /// with `?`, in any order.
    ///
    /// # Panics
    ///
    /// If the dark is not the map's sensor size.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::Cancelled`] if cancellation is requested before detection
    /// completes.
    pub fn detect_hot(
        mut self,
        dark: &CfaImage,
        sigma_threshold: f32,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        // Clamp at the boundary rather than asserting: `sigma_threshold` may come from user config,
        // and a non-positive value (which would flag every pixel above the median) must not panic
        // the pipeline. Nothing below 1σ is a meaningful defect threshold.
        let sigma_threshold = sigma_threshold.max(MIN_SIGMA_THRESHOLD);
        self.assert_dimensions(dark);
        self.hot_indices = detect_hot_pixels(dark, sigma_threshold, cancel)?;
        self.rebuild_mask();
        Ok(self)
    }

    /// Detect **cold/dead** pixels from a master flat — those reading below `DEAD_PIXEL_FRACTION`
    /// of their same-color local-neighbourhood median — and store them. The local reference makes
    /// this robust to vignetting and dust, where a global cut cannot be.
    ///
    /// # Panics
    ///
    /// If the flat is not the map's sensor size.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::Cancelled`] if cancellation is requested before detection
    /// completes.
    pub fn detect_cold(
        mut self,
        flat: &CfaImage,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        self.assert_dimensions(flat);
        self.cold_indices = detect_cold_pixels(flat, DEAD_PIXEL_FRACTION, cancel)?;
        self.rebuild_mask();
        Ok(self)
    }

    /// Every master feeding one map corrects the same sensor.
    fn assert_dimensions(&self, master: &CfaImage) {
        let size = master.size();
        assert!(
            size == self.dimensions,
            "a {size:?} master cannot feed a defect map for {:?}",
            self.dimensions
        );
    }

    fn rebuild_mask(&mut self) {
        self.mask = BitBuffer2::new_default(self.dimensions);
        for &index in self.hot_indices.iter().chain(&self.cold_indices) {
            self.mask.set(index, true);
        }
        self.count = self.mask.count_ones();
    }

    /// How many pixels are defective, hot or cold; a pixel that is both counts once.
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Percentage of the sensor's pixels that are defective.
    pub fn percentage(&self) -> f32 {
        100.0 * self.count as f32 / self.dimensions.pixel_count() as f32
    }

    /// Correct defective pixels on raw CFA data by replacing with median of
    /// same-color CFA neighbors, and flag them [`QualityFlags::DEFECT`] and [`QualityFlags::REPAIRED`]: the value
    /// left there is the neighbours', and a combine with other frames at that pixel can leave it
    /// out.
    ///
    /// # Panics
    ///
    /// If the image is not the map's sensor size.
    pub fn correct(&self, image: &mut CfaImage) {
        self.assert_dimensions(image);
        if self.count == 0 {
            return;
        }
        // Every defect is masked, so each repair draws only on good neighbours: a clustered
        // defect (hot column, adjacent same-color pixels) cannot pull a bad or half-corrected
        // value into a neighbour's median, and the order of the lists does not matter. A pixel
        // in both lists is repaired twice to the same value.
        let lattice = CfaLattice::new(&image.cfa_type);
        let mut scratch = Gathered::default();
        for &idx in self.hot_indices.iter().chain(&self.cold_indices) {
            image.data[idx] = lattice.median(
                &image.data,
                self.dimensions.point_of(idx),
                Some(&self.mask),
                &mut scratch,
            );
        }
        let mask = &self.mask;
        PixelFlags::add_where(
            &mut image.flags,
            self.dimensions,
            QualityFlags::DEFECT.union(QualityFlags::REPAIRED),
            |index| mask.get(index),
        );
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
) -> Result<Vec<usize>, CalibrationError> {
    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }

    let data = &image.data;
    let size = Size2us::new(data.width(), data.height());
    let total = size.pixel_count();
    let cfa_type = image.cfa_type;
    let background = ColourMesh::measure(
        data,
        &cfa_type,
        DARK_BACKGROUND_TILE_SIZE,
        &mut MeshWorkspace::default(),
    );
    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }
    let sigma_floor = residual_sigma_floor(image);
    let stats = compute_per_color_residual_stats(data, cfa_type, &background, sigma_floor);

    // The broad model reads each colour's tile skies rather than same-color neighbour medians, so a
    // compact same-color cluster remains an outlier instead of becoming its own local reference.
    let indices = (0..total)
        .into_par_iter()
        .filter(|&i| {
            if cancel.is_cancelled() {
                return false;
            }
            let point = size.point_of(i);
            let color = cfa_type.color_at(point) as usize;
            let ColorStats { median, sigma } = stats[color];
            data[i] - background.at(color, point).sky > median + sigma_threshold * sigma
        })
        .collect();

    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }
    Ok(indices)
}

fn residual_sigma_floor(image: &CfaImage) -> f32 {
    if let Some(sigma) = image
        .metadata
        .quantization_sigma
        .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
    {
        return sigma;
    }
    // One `f32` step at the frame's own magnitude — the smallest difference its samples can even
    // represent, so nothing below it is measurable whatever span the decoder divided by.
    let magnitude = image
        .data
        .par_iter()
        .map(|value| value.abs())
        .reduce(|| 0.0, f32::max);
    (magnitude * f32::EPSILON).max(f32::MIN_POSITIVE)
}

/// Flag cold/dead pixels in a master flat: those reading below `dead_fraction` of the median of
/// their same-color local neighbours. The local reference tracks vignetting (so a global cut's
/// negative-threshold failure can't happen) and ignores dust shadows; only near-zero pixels pass.
/// The neighbour scan runs on every pixel in parallel — one-time work, off the hot path.
fn detect_cold_pixels(
    image: &CfaImage,
    dead_fraction: f32,
    cancel: &CancelToken,
) -> Result<Vec<usize>, CalibrationError> {
    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }

    let data = &image.data;
    let size = Size2us::new(data.width(), data.height());
    let total = size.pixel_count();
    let lattice = CfaLattice::new(&image.cfa_type);

    let indices = (0..total)
        .into_par_iter()
        .map_init(Gathered::default, |scratch, i| {
            if cancel.is_cancelled() {
                return None;
            }
            let local = lattice.median(data, size.point_of(i), None, scratch);
            (data[i] < dead_fraction * local).then_some(i)
        })
        .flatten()
        .collect();

    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
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
    cfa_type: CfaType,
    background: &ColourMesh,
    sigma_floor: f32,
) -> ArrayVec<ColorStats, 3> {
    let num_colors = cfa_type.num_colors();
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

        // Leaves `samples` holding the absolute deviations, which the tail scale ranks.
        let MedianMad { median, mad } = MedianMad::of_mut(&mut samples);
        let tail_sigma = if samples.len() >= MIN_TAIL_SCALE_SAMPLES {
            let p99_index = (samples.len() - 1) * 99 / 100;
            let (_, p99, _) = samples.select_nth_unstable_by(p99_index, f32::total_cmp);
            *p99 * ABSOLUTE_RESIDUAL_P99_TO_SIGMA
        } else {
            0.0
        };
        let sigma = mad_to_sigma(mad).max(tail_sigma).max(sigma_floor);

        tracing::debug!(
            "Defect residual stats color={color}: median={median:.6}, MAD={mad:.6}, \
             tail_sigma={tail_sigma:.6}, floor={sigma_floor:.6}, sigma={sigma:.6}"
        );
        stats.push(ColorStats { median, sigma });
    }

    stats
}

#[cfg(all(test, feature = "bench"))]
mod bench;

#[cfg(test)]
mod tests;
