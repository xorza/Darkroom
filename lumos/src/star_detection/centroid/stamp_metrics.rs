//! [`StampMetrics`]: what a star's stamp measures of its light, whatever found its centre.

use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::stamp::{MAX_STAMP_PIXELS, StampAt};
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::roundness::{Roundness, RoundnessStamp};

/// What a star's metrics are measured against: the local sky the residual still carries, the noise
/// of the star's pixels, and how many samples the sky was measured from, when an annulus measured
/// it.
#[derive(Debug, Clone, Copy)]
pub(super) struct StarBg {
    pub(super) offset: f32,
    pub(super) noise: StarNoise,
    pub(super) sky_samples: Option<usize>,
}

/// A stamp's metrics against a background: flux, the star's own peak above the sky, the SNR by the
/// CCD equation, the sharpness of its own peak, and the DAOFIND roundness. The shape is measured
/// apart, by [`Self::moment_shape`], as a profile fit may replace it.
#[derive(Debug, Clone, Copy)]
pub(super) struct StampMetrics<'a> {
    pub(super) flux: f32,
    pub(super) peak: f32,
    pub(super) snr: f32,
    pub(super) sharpness: f32,
    pub(super) roundness: Roundness,
    stamp: StampAt<'a>,
    offset: f32,
    /// The second moments of the clipped signal about the stamp's centre.
    plain: Cov2,
}

impl<'a> StampMetrics<'a> {
    /// The metrics over `stamp` against `bg`, the roundness against a PSF of `psf_sigma`.
    /// `None` when the stamp leaves the frame or holds no star — no net or no positive light, a
    /// non-finite flux or SNR, or a marginal with no height.
    pub(super) fn measure(stamp: StampAt<'a>, psf_sigma: f64, bg: StarBg) -> Option<Self> {
        let StampAt {
            residual,
            pos,
            radius: stamp_radius,
        } = stamp;
        let offset = bg.offset;
        let centre = stamp.centre()?;

        // Flux and core flux sum the *signed* residual: sky noise is zero-mean, and clipping each
        // pixel at zero would turn it into a positive bias of about 0.4σ per pixel. The second
        // moments are weights, and a weight cannot be negative, so they take the clipped residual
        // and normalize by its own sum.
        let mut flux = 0.0f64;
        let mut weight_sum = 0.0f64;
        let mut core_flux = 0.0f64;
        let mut sum_x2 = 0.0f64;
        let mut sum_y2 = 0.0f64;
        let mut sum_xy = 0.0f64;
        let stamp_size = 2 * stamp_radius + 1;
        let mut values = [0.0f64; MAX_STAMP_PIXELS];

        for (my, y) in (centre.y - stamp_radius..=centre.y + stamp_radius).enumerate() {
            let px_row = residual.row(y);
            let ady = my.abs_diff(stamp_radius);
            for (mx, x) in (centre.x - stamp_radius..=centre.x + stamp_radius).enumerate() {
                let signal = f64::from(px_row[x] - offset);
                values[my * stamp_size + mx] = signal;
                let value = signal.max(0.0);
                flux += signal;
                weight_sum += value;
                if mx.abs_diff(stamp_radius) <= 1 && ady <= 1 {
                    core_flux += signal;
                }
                let fx = x as f64 - pos.x;
                let fy = y as f64 - pos.y;
                sum_x2 += value * fx * fx;
                sum_y2 += value * fy * fy;
                sum_xy += value * fx * fy;
            }
        }

        // No net signal above the sky, or nothing positive to weight the moments with: not a star.
        if flux < f64::EPSILON || weight_sum < f64::EPSILON {
            return None;
        }

        let npix = stamp_size * stamp_size;
        let snr = bg.noise.snr(flux, npix, bg.sky_samples);
        if !flux.is_finite() || !snr.is_finite() {
            return None;
        }
        // The star's own peak, at its centre pixel: the brightest pixel of the stamp may be a
        // companion's, and its sharpness then reads 1.
        let peak = values[stamp_radius * stamp_size + stamp_radius];
        let sharpness = if core_flux > 0.0 {
            (peak / core_flux).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        let roundness = Roundness::measure(RoundnessStamp {
            values: &values[..npix],
            radius: stamp_radius,
            psf_sigma,
        })?;

        Some(Self {
            flux: flux as f32,
            peak: peak as f32,
            snr: snr as f32,
            sharpness,
            roundness,
            stamp,
            offset,
            plain: Cov2 {
                xx: sum_x2 / weight_sum,
                yy: sum_y2 / weight_sum,
                xy: sum_xy / weight_sum,
            },
        })
    }

    /// The PSF's own shape as the stamp's moments read it, an axis narrower than a pixel at 0 —
    /// see [`Cov2::less_pixel`].
    ///
    /// Adaptive windowed second moments: Gaussian-weight by an iteratively matched window to
    /// suppress wing noise, then deconvolve the window so FWHM and eccentricity stay unbiased. The
    /// window is seeded from the plain moments, which stand where it cannot converge to a valid
    /// covariance. Both are measured against the offset the flux is, so the shape and the flux
    /// read one sky.
    pub(super) fn moment_shape(&self) -> Cov2 {
        Cov2::windowed(self.stamp, self.offset, self.plain.trace() / 2.0)
            .unwrap_or(self.plain)
            .less_pixel()
    }
}
