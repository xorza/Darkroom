//! Defective-pixel detection and correction.
//!
//! **Hot** pixels (abnormally high dark current) come from the master dark after subtracting a
//! robust per-color tiled background, then thresholding against a robust residual scale;
//! **cold/dead** pixels (abnormally low response) come from the master flat via a
//! local-neighbourhood ratio test. Both are corrected by replacing the pixel with the median of
//! its neighbours under the same filter: the same color, and on a Bayer sensor the same phase.
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
//!    On raw CFA data, hot pixels are replaced with the median of their neighbours under the same
//!    filter — for Bayer, the same phase, so a green in the red rows from greens in the red rows —
//!    which preserves the CFA pattern for demosaicing and carries no Gr/Gb imbalance into a repair.
//!
//! 4. **Broad dark structure:**
//!    Per-color tile medians are bilinearly interpolated into a smooth dark-current model before
//!    thresholding, which keeps isolated pixels and same-color clusters as positive residuals. A
//!    pixel the model calls hot is confirmed against a robust plane through its color's photosites
//!    6 to 8 pixels around it ([`RingReference`]), so amp glow that
//!    curves faster than the tiles, or rises past the outer ones, does not become a defect.
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

pub(crate) mod defect_summary;
pub(crate) mod ring_reference;
pub(crate) mod sampling;
pub(crate) mod sigma_threshold;

use arrayvec::ArrayVec;
use common::{CancelToken, PerWorker, par};
use rayon::prelude::*;

use crate::bg_mesh::color_mesh::ColorMesh;
use crate::bit_buffer2::BitBuffer2;
use crate::calibration::defect_map::defect_summary::{ColorDefects, ColorShares, DefectSummary};
use crate::calibration::defect_map::ring_reference::{RingFrame, RingReference};
use crate::calibration::defect_map::sampling::ColorResiduals;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::error::CalibrationError;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_lattice::{CfaLattice, Gathered};
use crate::image::cfa::cfa_type::CfaType;
use crate::image::masked_plane::MaskedPlane;
use crate::image::quality_flags::QualityFlags;
use crate::math::statistics::median_mad::MedianMad;
use common::Size2us;

/// A mask of defective pixels: **hot** pixels (abnormally high dark current) from a master
/// dark, and **cold/dead** pixels (abnormally low response) from a master flat.
///
/// Each is detected per CFA color and replaced with the median of its neighbours under the same
/// filter during correction. The two defects come from *different* masters by necessity: a dark has
/// no illumination, so dead pixels are invisible in it (they read the same near-zero as a normal
/// dark pixel) — they only reveal themselves as dark spots in an illuminated flat.
#[derive(Debug, Clone)]
pub(crate) struct DefectMap {
    hot: HotPixels,
    cold_idxs: Vec<usize>,
    /// Every defect, hot or cold, once: what a repair keeps out of its neighbours, built when
    /// the lists change rather than for every light corrected.
    mask: BitBuffer2,
    /// Pixels set in `mask` — a pixel both hot and dead counts once.
    count: usize,
    dimensions: Size2us,
}

/// The hot pixels of a dark: each one's flat index, and how far it stands above the dark around it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct HotPixels {
    pub(crate) idxs: Vec<usize>,
    /// Per index, in the dark's units: its residual over the robust plane through the ring of its
    /// color's photosites around it, or over the color's mesh background where the ring has too
    /// few — the residual the detection tested. Always positive.
    pub(crate) excess: Vec<f32>,
}

impl DefectMap {
    /// An empty map for a sensor of `dimensions`, ready for [`Self::detect_hot`] and
    /// [`Self::detect_cold`].
    pub(crate) fn new(dimensions: Size2us) -> Self {
        Self {
            hot: HotPixels::default(),
            cold_idxs: Vec::new(),
            mask: BitBuffer2::new_default(dimensions),
            count: 0,
            dimensions,
        }
    }

    /// A map with these defect lists, or `None` when an index lies outside `dimensions`.
    pub(crate) fn from_lists(
        dimensions: Size2us,
        hot: HotPixels,
        cold_idxs: Vec<usize>,
    ) -> Option<Self> {
        debug_assert_eq!(hot.idxs.len(), hot.excess.len());
        let pixel_count = dimensions.pixel_count();
        if hot
            .idxs
            .iter()
            .chain(&cold_idxs)
            .any(|&idx| idx >= pixel_count)
        {
            return None;
        }
        let mut map = Self::new(dimensions);
        map.hot = hot;
        map.cold_idxs = cold_idxs;
        map.rebuild_mask();
        Some(map)
    }

    /// The sensor extent the indices apply to.
    pub(crate) const fn dimensions(&self) -> Size2us {
        self.dimensions
    }

    /// The hot pixels — above `median + kσ` in the background-subtracted dark — with their excess.
    pub(crate) const fn hot(&self) -> &HotPixels {
        &self.hot
    }

    /// Flat indices of cold/dead pixels — below `DEAD_PIXEL_FRACTION` of their same-color
    /// local-neighbourhood median in the flat.
    pub(crate) fn cold_idxs(&self) -> &[usize] {
        &self.cold_idxs
    }

    /// Resident RAM held by the map: its hot pixels, its cold index list and its mask.
    pub(crate) const fn ram_bytes(&self) -> usize {
        self.hot.idxs.len() * (size_of::<usize>() + size_of::<f32>())
            + self.cold_idxs.len() * size_of::<usize>()
            + BitBuffer2::heap_bytes(self.mask.size)
    }

    /// How many pixels the map names, in all and per color of `cfa`, the filter of the masters it
    /// was detected on.
    pub(crate) fn summary(&self, cfa: &CfaType) -> DefectSummary {
        let photosites = cfa.photosites_per_color(self.dimensions);
        let of = |photosites| ColorDefects {
            photosites,
            ..ColorDefects::default()
        };
        let mut colors = match photosites.as_slice() {
            &[mono] => ColorShares::Mono(of(mono)),
            &[red, green, blue] => ColorShares::Mosaic([of(red), of(green), of(blue)]),
            other => unreachable!("a sensor has one color or three, not {}", other.len()),
        };
        let color_of = |idx: usize| cfa.slot_at(self.dimensions.point_of(idx));
        for &idx in &self.hot.idxs {
            colors.slot_mut(color_of(idx)).hot += 1;
        }
        for &idx in &self.cold_idxs {
            colors.slot_mut(color_of(idx)).cold += 1;
        }
        self.mask
            .for_each_set(|point| colors.slot_mut(cfa.slot_at(point)).defective += 1);
        DefectSummary {
            hot_pixels: self.hot.idxs.len(),
            cold_pixels: self.cold_idxs.len(),
            defective_pixels: self.count,
            pixels: self.dimensions.pixel_count(),
            colors,
        }
    }

    /// Detect **hot** pixels from a master dark — those whose residual above a smooth per-color
    /// dark background exceeds `median + sigma_threshold·σ` — and store them. Calls are chainable
    /// with `?`, in any order.
    ///
    /// The dark is the map's sensor size.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::Cancelled`] if cancellation is requested before detection
    /// completes.
    pub(crate) fn detect_hot(
        mut self,
        dark: &CfaImage,
        sigma_threshold: SigmaThreshold,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        self.assert_dimensions(dark);
        self.hot = detect_hot_pixels(dark, sigma_threshold.get(), cancel)?;
        self.rebuild_mask();
        Ok(self)
    }

    /// Detect **cold/dead** pixels from a master flat — those reading below `DEAD_PIXEL_FRACTION`
    /// of their same-color local-neighbourhood median — and store them. The local reference makes
    /// this robust to vignetting and dust, where a global cut cannot be.
    ///
    /// The flat is the map's sensor size.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::Cancelled`] if cancellation is requested before detection
    /// completes.
    pub(crate) fn detect_cold(
        mut self,
        flat: &CfaImage,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        self.assert_dimensions(flat);
        self.cold_idxs = detect_cold_pixels(flat, DEAD_PIXEL_FRACTION, cancel)?;
        self.rebuild_mask();
        Ok(self)
    }

    /// Every master feeding one map corrects the same sensor.
    fn assert_dimensions(&self, master: &CfaImage) {
        let size = master.size();
        debug_assert!(
            size == self.dimensions,
            "a {size:?} master cannot feed a defect map for {:?}",
            self.dimensions
        );
    }

    fn rebuild_mask(&mut self) {
        self.mask = BitBuffer2::new_default(self.dimensions);
        for &idx in self.hot.idxs.iter().chain(&self.cold_idxs) {
            self.mask.set(idx, true);
        }
        self.count = self.mask.count_ones();
    }

    /// Correct defective pixels on raw CFA data by replacing with median of
    /// same-color CFA neighbors, and flag them [`QualityFlags::DEFECT`] and
    /// [`QualityFlags::REPAIRED`]: the value left there is the neighbours', and a combine with
    /// other frames at that pixel can leave it out.
    ///
    /// The image is the map's sensor size, and `unmeasured` its [`CfaImage::unmeasured_mask`],
    /// which the defects join: the mask then covers every pixel no later repair may read either.
    pub(crate) fn correct(&self, image: &mut CfaImage, unmeasured: Option<&mut BitBuffer2>) {
        self.assert_dimensions(image);
        if self.count == 0 {
            return;
        }
        // Every defect is masked, and every pixel that holds no measurement, so each repair draws
        // only on measured neighbours: a clustered defect (hot column, adjacent same-color pixels)
        // cannot pull a bad or half-corrected value into a neighbour's median, a fill or another
        // repair cannot pass for a measurement, and the order of the lists does not matter. A pixel
        // in both lists is repaired twice to the same value.
        let mask = match unmeasured {
            Some(unmeasured) => {
                unmeasured.or_with(&self.mask);
                &*unmeasured
            }
            None => &self.mask,
        };
        let lattice = CfaLattice::new(&image.cfa_type());
        let mut scratch = Gathered::default();
        for &idx in self.hot.idxs.iter().chain(&self.cold_idxs) {
            let plane = MaskedPlane {
                pixels: image.data(),
                mask: Some(mask),
            };
            image.data_mut()[idx] =
                lattice.median(plane, self.dimensions.point_of(idx), &mut scratch);
        }
        for idxs in [&self.hot.idxs, &self.cold_idxs] {
            image.add_flags_at(QualityFlags::DEFECT.union(QualityFlags::REPAIRED), idxs);
        }
    }
}

/// Maximum number of samples per color channel for median estimation.
const MAX_MEDIAN_SAMPLES: usize = 100_000;

/// Broad dark-current model tile size. Each tile has enough Bayer red/blue samples for a robust
/// median while remaining much smaller than normal sensor-scale gradients and amp glow.
const DARK_BG_TILE_SIZE: usize = 64;

/// `1 / Φ⁻¹(0.99)`: the 1st percentile of `N(m, σ)` lies `Φ⁻¹(0.99)·σ` below `m`, so this turns
/// that distance back into σ.
const LOWER_RESIDUAL_P1_TO_SIGMA: f32 = 0.429_858_32;
// Five expected tail samples keep one sparse defect from defining the scale on tiny images.
const MIN_TAIL_SCALE_SAMPLES: usize = 500;

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
) -> Result<HotPixels, CalibrationError> {
    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }

    let data = image.data();
    let size = data.size();
    let cfa_type = image.cfa_type();
    let bg = ColorMesh::measure(MaskedPlane::from(data), &cfa_type, DARK_BG_TILE_SIZE);
    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }
    let sigma_floor = residual_sigma_floor(image);
    let residuals = ColorResiduals {
        data,
        cfa_type,
        bg: &bg,
    };
    let stats = compute_per_color_residual_stats(residuals, sigma_floor);

    // The broad model reads each color's tile skies rather than same-color neighbour medians, so a
    // compact same-color cluster remains an outlier instead of becoming its own local reference.
    // What it calls hot must stand as far above the ring's plane, which follows the structure the
    // tiles cannot. A row a task, its worker's ring taken once for the row's pixels.
    let rings = PerWorker::new(RingReference::default);
    let (idxs, excess) = par::into_iter(0..size.height)
        .flat_map_iter(|y| {
            let mut ring = rings.get();
            let (stats, cfa_type, bg) = (&stats, &cfa_type, &bg);
            (y * size.width..(y + 1) * size.width).filter_map(move |i| {
                if cancel.is_cancelled() {
                    return None;
                }
                let point = size.point_of(i);
                let color = cfa_type.slot_at(point);
                let ColorStats { median, sigma } = stats[color];
                let local = bg.at(color, point);
                let cut = sigma_threshold * sigma.max(local.noise);
                let residual = data[i] - local.sky;
                if residual <= median + cut {
                    return None;
                }
                match ring.at(RingFrame::of(data), cfa_type, point) {
                    Some(level) => {
                        let excess = data[i] - level;
                        (excess > cut).then_some((i, excess))
                    }
                    None => Some((i, residual - median)),
                }
            })
        })
        .unzip();

    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }
    Ok(HotPixels { idxs, excess })
}

fn residual_sigma_floor(image: &CfaImage) -> f32 {
    if let Some(sigma) = image
        .metadata
        .derived
        .quantization_sigma()
        .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
    {
        return sigma;
    }
    // One `f32` step at the frame's own magnitude — the smallest difference its samples can even
    // represent, so nothing below it is measurable whatever span the decoder divided by.
    let magnitude = par::iter(image.data())
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

    let data = image.data();
    let size = data.size();
    let lattice = CfaLattice::new(&image.cfa_type());

    let gathered = PerWorker::new(Gathered::default);
    // A row a task, its worker's scratch taken once for the row's pixels.
    let idxs = par::into_iter(0..size.height)
        .flat_map_iter(|y| {
            let mut scratch = gathered.get();
            let lattice = &lattice;
            (y * size.width..(y + 1) * size.width).filter(move |&i| {
                if cancel.is_cancelled() {
                    return false;
                }
                let plane = MaskedPlane::from(data);
                let local = lattice.median(plane, size.point_of(i), &mut scratch);
                data[i] < dead_fraction * local
            })
        })
        .collect();

    if cancel.is_cancelled() {
        return Err(CalibrationError::Cancelled);
    }
    Ok(idxs)
}

/// Per-CFA-color robust residual statistics used to threshold hot pixels.
#[derive(Debug, Clone, Copy)]
struct ColorStats {
    /// Median residual for the color (the hot-detection center).
    median: f32,
    /// Robust σ from MAD and the lower residual tail, resolution-floored. No samples gives `∞`.
    sigma: f32,
}

/// Per-CFA-color robust background-subtracted stats, indexed by color (0=R/mono, 1=G, 2=B).
///
/// `sigma` takes the larger of MAD and the Gaussian-calibrated distance from the median down to the
/// 1st residual percentile. The latter keeps broad model error and column structure, which spread
/// a residual both ways, out of the defect tail; a hot pixel only reads high, so the lower tail
/// holds none of them whatever their density. (The upper tail, or the absolute one, falls inside
/// the defects once they pass 1% of a color, as they do on an uncooled sensor's dark.) The result
/// is floored at the master image's quantization/numeric resolution so a zero-MAD plateau does not
/// turn every representable deviation into a defect. A color with no samples gets `sigma = ∞` so
/// it never flags.
fn compute_per_color_residual_stats(
    residuals: ColorResiduals<'_>,
    sigma_floor: f32,
) -> ArrayVec<ColorStats, 3> {
    let color_count = residuals.cfa_type.color_count();
    let mut stats = ArrayVec::new();

    for color in 0..color_count {
        let mut samples = residuals.samples(color);

        if samples.is_empty() {
            stats.push(ColorStats {
                median: 0.0,
                sigma: f32::INFINITY,
            });
            continue;
        }

        let lower_tail = (samples.len() >= MIN_TAIL_SCALE_SAMPLES).then(|| {
            let p1_idx = (samples.len() - 1) / 100;
            *samples.select_nth_unstable_by(p1_idx, f32::total_cmp).1
        });
        let residual = MedianMad::of_mut(&mut samples);
        let MedianMad { median, mad, .. } = residual;
        let tail_sigma = lower_tail.map_or(0.0, |p1| (median - p1) * LOWER_RESIDUAL_P1_TO_SIGMA);
        let sigma = residual.sigma().max(tail_sigma).max(sigma_floor);

        tracing::debug!(
            "Defect residual stats color={color}: median={median:.6}, MAD={mad:.6}, \
             tail_sigma={tail_sigma:.6}, floor={sigma_floor:.6}, sigma={sigma:.6}"
        );
        stats.push(ColorStats { median, sigma });
    }

    stats
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
