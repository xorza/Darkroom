//! The square of pixels one star is measured over.
//!
//! Every measurement — moments, both profile fits, the covariance — works on a stamp centred on
//! the candidate rather than on the frame, so the stamp is where the shared setup lives: the
//! local coordinate grid (built once per detection, not per star), the extracted
//! background-subtracted samples, and the per-pixel inverse-variance weights a noise model
//! implies.

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::stacking::star_detection::centroid::lm_optimizer::FitData;
use crate::stacking::star_detection::centroid::{MAX_STAMP_PIXELS, is_valid_stamp_position};
use crate::stacking::star_detection::config::measurement_config::NoiseModel;

/// The stamp's own pixel coordinates, `0..2r` on each axis, flattened row-major.
///
/// Identical for every candidate of a given radius, so it is built once per detection rather than
/// per star: both profile models read the data only through `x - x0` and `y - y0`, so fitting in
/// stamp-local coordinates and shifting the centre back afterwards is the same fit. The smaller
/// magnitudes also condition the normal equations a little better than image coordinates do.
#[derive(Debug)]
pub(crate) struct StampGrid {
    pub(super) x: ArrayVec<f64, MAX_STAMP_PIXELS>,
    pub(super) y: ArrayVec<f64, MAX_STAMP_PIXELS>,
    pub(crate) radius: usize,
}

impl StampGrid {
    pub(crate) fn new(radius: usize) -> Self {
        let side = 2 * radius + 1;
        let mut x = ArrayVec::new();
        let mut y = ArrayVec::new();
        for row in 0..side {
            for column in 0..side {
                x.push(column as f64);
                y.push(row as f64);
            }
        }
        Self { x, y, radius }
    }
}

/// Stack-allocated stamp data extracted around a star candidate.
/// Uses ArrayVec to avoid heap allocations for typical stamp sizes.
#[derive(Debug)]
pub(super) struct StampData {
    /// Pixel values (background-subtracted at the caller if needed), row-major over the stamp.
    /// The matching coordinates live in the shared [`StampGrid`].
    pub(super) z: ArrayVec<f64, MAX_STAMP_PIXELS>,
    /// Peak pixel value within the stamp.
    pub(super) peak: f32,
    /// Image position of the stamp's top-left pixel, which [`StampGrid`]'s coordinates are
    /// relative to. Integer-valued, but f64 so shifting a fitted centre back into image
    /// coordinates costs no rounding.
    pub(super) origin: DVec2,
}

/// Noise inputs for an inverse-variance-weighted fit: the local sky σ plus the
/// normalized-domain sensor model. `None` (absent) means an unweighted fit.
#[derive(Debug, Clone, Copy)]
pub(super) struct FitNoise {
    pub(super) sky_noise: f32,
    pub(super) noise_model: NoiseModel,
}

impl FitNoise {
    /// Inverse-variance weight for one pixel, using the CCD noise model (the same per-pixel
    /// decomposition as `compute_snr`):
    /// `w = 1 / (signal/G + sky_noise² + (read_noise_electrons/G)²)`, where `G` is electrons per
    /// normalized unit.
    ///
    /// Down-weights the shot-noisy bright core so the fit is the ML estimator instead of
    /// over-weighting high-signal pixels (which biases the sub-pixel centroid/FWHM/flux). The
    /// variance is floored so a zero-variance pixel cannot produce an infinite weight.
    #[inline]
    pub(super) fn weight(&self, z: f64, background: f64) -> f64 {
        let signal = (z - background).max(0.0);
        1.0 / self
            .noise_model
            .variance_normalized(signal, self.sky_noise as f64, 1)
            .max(1e-12)
    }
}

/// Gaussian-equivalent width from a stamp's weighted second moments: for a Gaussian
/// `E[r²] = 2σ²`, so `σ = sqrt(E[r²]/2)`. A better seed for L-M than a fixed value.
///
/// `max_sigma` is the widest profile the optimizer will accept, so the seed lands inside the
/// range its own `constrain` enforces — starting outside it just spends the first iteration
/// being clamped back. Always at least [`MIN_STAMP_RADIUS`](super::MIN_STAMP_RADIUS), so the 0.5 floor cannot cross it.
pub(super) fn sigma_from_moments(sum_r2: f64, sum_w: f64, max_sigma: f64) -> f32 {
    if sum_w > f64::EPSILON {
        (sum_r2 / sum_w / 2.0).sqrt().clamp(0.5, max_sigma) as f32
    } else {
        2.0
    }
}

/// The per-candidate inputs both profile fits need before the optimizer runs: the stamp, the
/// centre expressed in the stamp's own frame, the inverse-variance weights, and a width seed.
///
/// The two fits differ only in the model they hand to the optimizer and the parameters they read
/// back out; everything up to that point is this.
#[derive(Debug)]
pub(super) struct StampFit {
    pub(super) stamp: StampData,
    /// `pos` relative to [`StampData::origin`] — the frame the fit runs in.
    pub(super) local_pos: DVec2,
    pub(super) weights: Option<ArrayVec<f64, MAX_STAMP_PIXELS>>,
    /// Gaussian-equivalent width from the stamp's second moments, seeding the optimizer.
    pub(super) sigma_est: f32,
}

impl StampFit {
    /// `None` when the stamp falls outside the frame, or holds too few pixels to constrain `N`
    /// free parameters — a least-squares fit needs strictly more samples than parameters.
    ///
    /// One pass produces all four outputs. Pixel values, the peak, the second moments behind
    /// `sigma_est` and the inverse-variance weights used to be an extraction walk plus one walk per
    /// consumer — three traversals of the same 225 f64 per candidate, on the hottest path in
    /// `measure_star`.
    pub(super) fn prepare<const N: usize>(
        pixels: &Buffer2<f32>,
        pos: DVec2,
        grid: &StampGrid,
        background: f32,
        noise: Option<FitNoise>,
    ) -> Option<Self> {
        let radius = grid.radius;
        if !is_valid_stamp_position(pos, Size2us::new(pixels.width(), pixels.height()), radius) {
            return None;
        }
        let stamp_size = 2 * radius + 1;
        if stamp_size * stamp_size <= N {
            return None;
        }

        let icx = pos.x.round() as isize;
        let icy = pos.y.round() as isize;
        // Moment arms measured from the rounded centre: at column `dx` the offset from the true
        // centre is `dx - frac_x`, which is the shared grid's coordinate minus `local_pos` without
        // going through either array.
        let frac_x = pos.x - icx as f64;
        let frac_y = pos.y - icy as f64;
        let sky = background as f64;

        let mut z = ArrayVec::new();
        // Filled only for a weighted fit, but an empty `ArrayVec` costs nothing to carry: its
        // backing storage is uninitialized until pushed to.
        let mut weights = ArrayVec::new();
        let mut peak = f32::MIN;
        let mut sum_r2 = 0.0f64;
        let mut sum_w = 0.0f64;

        let radius_i32 = radius as i32;
        for dy in -radius_i32..=radius_i32 {
            let y = (icy + dy as isize) as usize;
            // One bounds check per row rather than per pixel — the guard above has already
            // established the whole stamp is inside the frame.
            let row = pixels.row(y);
            let ddy = dy as f64 - frac_y;

            for dx in -radius_i32..=radius_i32 {
                let value = row[(icx + dx as isize) as usize];
                let value64 = f64::from(value);
                z.push(value64);
                peak = peak.max(value);

                let signal = (value64 - sky).max(0.0);
                let ddx = dx as f64 - frac_x;
                sum_r2 += signal * (ddx * ddx + ddy * ddy);
                sum_w += signal;

                if let Some(n) = noise {
                    weights.push(n.weight(value64, sky));
                }
            }
        }

        let origin = DVec2::new(
            (icx - radius as isize) as f64,
            (icy - radius as isize) as f64,
        );
        Some(Self {
            stamp: StampData { z, peak, origin },
            // Fit in the stamp's own frame: the models are translation-invariant, so this is the
            // same fit with better-conditioned magnitudes, and the coordinate arrays become the
            // shared grid instead of two per-candidate ramps.
            local_pos: pos - origin,
            weights: noise.map(|_| weights),
            sigma_est: sigma_from_moments(sum_r2, sum_w, radius as f64),
        })
    }

    /// The optimizer's view of the stamp: coordinate ramps shared across the whole detection,
    /// pixel values and weights owned per candidate.
    pub(super) fn data<'a>(&'a self, grid: &'a StampGrid) -> FitData<'a> {
        FitData::new(&grid.x, &grid.y, &self.stamp.z, self.weights.as_deref())
    }

    /// Amplitude seed: the stamp's peak above the sky, floored so the optimizer starts positive.
    pub(super) fn amplitude_seed(&self, background: f32) -> f64 {
        (self.stamp.peak - background).max(0.01) as f64
    }

    /// Lift a fitted centre out of the stamp frame back into image coordinates.
    pub(super) fn to_image(&self, x0: f64, y0: f64) -> DVec2 {
        DVec2::new(x0, y0) + self.stamp.origin
    }
}
