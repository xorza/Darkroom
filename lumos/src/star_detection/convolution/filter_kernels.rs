//! [`FilterKernels`]: the matched filter, over kernel weights it keeps from frame to frame.

use std::mem;

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::concurrency::SAMPLES_PER_BLOCK;
use crate::math::fwhm;
use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};
use crate::star_detection::convolution;
use crate::star_detection::convolution::simd::Kernel2d;
use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;

/// Maximum deviation of `axis_ratio` from 1.0 to use the faster separable
/// (circular) kernel path instead of full 2D elliptical convolution.
const CIRCULAR_KERNEL_THRESHOLD: f32 = 0.01;

/// How closely an elliptical kernel's weights are each its profile's mean over a pixel, relative to
/// the peak: half an ulp of the f32 weights.
const KERNEL_MEAN_TOLERANCE: f64 = 1.0 / (1u64 << 25) as f64;

/// The most cells a pixel is divided into along each axis for its mean: 16 lets the 16-point rule
/// resolve a profile of σ 0.01 px across the pixel axes, far thinner than a star.
const MAX_PIXEL_SUBDIVISION: usize = 16;

/// How an elliptical kernel takes each pixel's mean: over `subdivision × subdivision` cells, each
/// by the Gauss–Legendre rule of `order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelRule {
    pub(crate) subdivision: usize,
    pub(crate) order: usize,
}

/// The matched filter's kernel weights, kept from frame to frame: the row and column kernels of
/// a separable filter, or the 2D kernel in `rows`.
#[derive(Debug, Default)]
pub(crate) struct FilterKernels {
    rows: Vec<f32>,
    cols: Vec<f32>,
}

/// An elliptical Gaussian: σ along its major axis, the minor-to-major ratio, and the major axis's
/// angle to the x axis, in radians.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EllipticalGaussian {
    pub(super) sigma: f32,
    pub(super) axis_ratio: f32,
    pub(super) angle: f32,
}

/// What the matched filter convolves with: a Gaussian of `[σx, σy]` along the pixel axes, filtered
/// in a row pass and a column pass, or an ellipse at another angle, filtered with a 2D kernel.
#[derive(Debug, Clone, Copy)]
pub(crate) enum KernelShape {
    Separable([f32; 2]),
    Elliptical(EllipticalGaussian),
}

impl KernelShape {
    /// The shape `filter` convolves with. An ellipse along the pixel axes is a product of two 1D
    /// profiles, and takes them exactly; at any other angle an axis ratio this close to 1 takes the
    /// separable circular kernel.
    pub(crate) fn of(filter: MatchedFilter) -> Self {
        let MatchedFilter {
            fwhm,
            axis_ratio,
            angle,
        } = filter;
        let sigma = fwhm::to_sigma(fwhm);
        match convolution::quarter_turns(angle) {
            Some(turns) => {
                let minor = sigma * axis_ratio;
                Self::Separable(if turns % 2 == 0 {
                    [sigma, minor]
                } else {
                    [minor, sigma]
                })
            }
            None if (axis_ratio - 1.0).abs() < CIRCULAR_KERNEL_THRESHOLD => {
                Self::Separable([sigma, sigma])
            }
            None => Self::Elliptical(EllipticalGaussian {
                sigma,
                axis_ratio,
                angle,
            }),
        }
    }
}

impl FilterKernels {
    /// Apply matched filter convolution optimized for star detection, in place, with `temp` — of
    /// any contents and `values`' size — for the intermediate pass.
    ///
    /// Convolves the residual — the image less its sky, so the convolution carries no pedestal —
    /// with a Gaussian kernel matching the expected PSF as the pixels record it: each weight is the
    /// PSF's mean over its pixel. The output is normalized by `sqrt(sum(K^2))`, which keeps white
    /// noise at its σ; the detection plane measures the σ its output has anyway, which holds for
    /// noise of any correlation.
    ///
    /// SEP's matched filter (Barbary 2016) where the noise is uniform:
    /// `SNR = conv(D, K) / (sigma * sqrt(sum(K^2)))`. Where σ varies, SEP weighs each pixel by
    /// `1/σ²` inside the sums; this does not, and the threshold reads the local σ of the output
    /// instead.
    ///
    /// Supports elliptical PSF shapes for stars elongated due to tracking errors,
    /// field rotation, or optical aberrations. For circular PSFs, use `axis_ratio = 1.0`. An
    /// ellipse whose axes lie along the pixel axes is a product of two 1D profiles, and so is its
    /// pixel mean, so it filters in a row pass and a column pass. At any other angle the kernel is
    /// 2D: Geusebroek, van den Boomgaard and Smeulders (2003) split a rotated Gaussian into an axis
    /// pass and a skewed one, but the skewed pass interpolates between pixels, and the kernel is
    /// then no longer the PSF's pixel mean.
    pub(crate) fn matched_filter(
        &mut self,
        values: &mut Buffer2<f32>,
        filter: MatchedFilter,
        temp: &mut Buffer2<f32>,
    ) {
        debug_assert_eq!(values.width(), temp.width());
        debug_assert_eq!(values.height(), temp.height());
        debug_assert!(
            filter.axis_ratio > 0.0 && filter.axis_ratio <= 1.0,
            "Axis ratio must be in (0, 1]"
        );

        // sqrt(sum(K²)) of the kernel the convolution used.
        let noise_norm = match KernelShape::of(filter) {
            KernelShape::Separable(sigmas) => self.separable(values, sigmas, temp),
            KernelShape::Elliptical(ellipse) => {
                let norm = self.elliptical(values, ellipse, temp);
                mem::swap(values, temp);
                norm
            }
        };

        // After convolution the noise is the map's times sqrt(sum(K²)); dividing it out puts the
        // filtered image back on the map's scale.
        let inv_norm = 1.0 / noise_norm;
        values
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .for_each(|block| block.iter_mut().for_each(|px| *px *= inv_norm));
    }

    /// Convolve `values` in place with the Gaussian of `[σx, σy]` along the axes, out to `3σ` of
    /// the wider both ways, using `temp` for the row pass.
    ///
    /// Separable: the rows, then the columns, O(n×k) instead of O(n×k²). Both passes mirror at the
    /// edges per axis, so they hold for a kernel wider than the image too. Returns `sqrt(sum(K²))`
    /// of the equivalent normalized 2D kernel, `sqrt(Σkx²·Σky²)`: for a circular kernel, `Σk²`
    /// exactly.
    pub(super) fn separable(
        &mut self,
        values: &mut Buffer2<f32>,
        [sigma_x, sigma_y]: [f32; 2],
        temp: &mut Buffer2<f32>,
    ) -> f32 {
        debug_assert!(sigma_x > 0.0 && sigma_y > 0.0, "Sigma must be positive");
        debug_assert_eq!(values.width(), temp.width());
        debug_assert_eq!(values.height(), temp.height());

        let radius = convolution::kernel_radius(sigma_x.max(sigma_y));
        convolution::fill_gaussian_kernel_1d(sigma_x, radius, &mut self.rows);
        convolution::fill_gaussian_kernel_1d(sigma_y, radius, &mut self.cols);
        // The column pass reads the row pass's output alone, so it writes back over the input.
        convolution::convolve_rows_parallel(values, temp, &self.rows);
        convolution::convolve_cols(temp, values, &self.cols);
        let squares = |kernel: &[f32]| kernel.iter().map(|&weight| weight * weight).sum::<f32>();
        (squares(&self.rows) * squares(&self.cols)).sqrt()
    }

    /// Convolve `pixels` with the elliptical Gaussian `ellipse` into `output`.
    ///
    /// Full 2D convolution, O(n×k²), for an ellipse at an angle to the pixel axes. Returns
    /// `sqrt(sum(K²))` for the normalized 2D kernel.
    pub(super) fn elliptical(
        &mut self,
        pixels: &Buffer2<f32>,
        ellipse: EllipticalGaussian,
        output: &mut Buffer2<f32>,
    ) -> f32 {
        debug_assert_eq!(pixels.width(), output.width());
        debug_assert_eq!(pixels.height(), output.height());

        let size = ellipse.fill_kernel(&mut self.rows);
        convolution::convolve_2d(pixels, Kernel2d::new(&self.rows, size), output);
        self.rows
            .iter()
            .map(|&weight| weight * weight)
            .sum::<f32>()
            .sqrt()
    }
}

impl EllipticalGaussian {
    /// The narrowest σ the ellipse has along either pixel axis at a fixed offset along the other:
    /// `1/√P` for the larger diagonal entry `P` of its inverse covariance. Near `σ·axis_ratio` for a
    /// thin ellipse off the axes, it is what the pixel quadrature has to resolve.
    fn axis_sigma(self) -> f64 {
        let major = f64::from(self.sigma);
        let minor = major * f64::from(self.axis_ratio);
        let (sin, cos) = f64::from(self.angle).sin_cos();
        let (major_precision, minor_precision) = (1.0 / (major * major), 1.0 / (minor * minor));
        let along_x = cos * cos * major_precision + sin * sin * minor_precision;
        let along_y = sin * sin * major_precision + cos * cos * minor_precision;
        1.0 / along_x.max(along_y).sqrt()
    }

    /// The fewest cells per pixel, and the order for them, whose pixel means are within
    /// [`KERNEL_MEAN_TOLERANCE`]: a cell of width `1/s` sees the profile `s` times wider, so the
    /// rule that reaches `s·σ` integrates it. `None` past [`MAX_PIXEL_SUBDIVISION`]: an ellipse too
    /// thin across the pixel axes to be any star's.
    pub(crate) fn pixel_rule(self) -> Option<PixelRule> {
        let sigma = self.axis_sigma();
        (1..=MAX_PIXEL_SUBDIVISION).find_map(|subdivision| {
            PixelQuadrature::sufficient_order(
                AnalyticProfile::Gaussian {
                    sigma: sigma * subdivision as f64,
                },
                KERNEL_MEAN_TOLERANCE,
            )
            .map(|order| PixelRule { subdivision, order })
        })
    }

    /// Into `weights`, row by row, the 2D kernel out to `3σ` of the major axis, each weight the
    /// profile's mean over its pixel by [`Self::pixel_rule`], normalized to sum to 1; returns its
    /// side.
    pub(super) fn fill_kernel(self, weights: &mut Vec<f32>) -> usize {
        let Self {
            sigma,
            axis_ratio,
            angle,
        } = self;
        debug_assert!(sigma > 0.0, "Sigma must be positive");
        debug_assert!(
            axis_ratio > 0.0 && axis_ratio <= 1.0,
            "Axis ratio must be in (0, 1]"
        );

        let radius = convolution::kernel_radius(sigma);
        let size = 2 * radius + 1;

        let sigma_major = f64::from(sigma);
        let sigma_minor = sigma_major * f64::from(axis_ratio);
        let (sin_a, cos_a) = f64::from(angle).sin_cos();
        let two_sigma_major_sq = 2.0 * sigma_major * sigma_major;
        let two_sigma_minor_sq = 2.0 * sigma_minor * sigma_minor;
        let profile = |x: f64, y: f64| {
            let x_rot = x * cos_a + y * sin_a;
            let y_rot = -x * sin_a + y * cos_a;
            (-x_rot * x_rot / two_sigma_major_sq - y_rot * y_rot / two_sigma_minor_sq).exp()
        };

        let PixelRule { subdivision, order } = self
            .pixel_rule()
            .expect("validation keeps the matched filter's ellipse within the pixel rules");
        let rule = PixelQuadrature::gauss_legendre(order);
        let cell = 1.0 / subdivision as f64;
        let mean = |index: usize| {
            let x = (index % size) as f64 - radius as f64;
            let y = (index / size) as f64 - radius as f64;
            let mut sum = 0.0;
            for row in 0..subdivision {
                let cell_y = y - 0.5 + (row as f64 + 0.5) * cell;
                for column in 0..subdivision {
                    let cell_x = x - 0.5 + (column as f64 + 0.5) * cell;
                    sum += rule.integrate(0.0, 0.0, |u, v| {
                        profile(cell_x + u * cell, cell_y + v * cell)
                    });
                }
            }
            sum * cell * cell
        };
        let sum: f64 = (0..size * size).map(mean).sum();
        weights.clear();
        weights.extend((0..size * size).map(|index| (mean(index) / sum) as f32));
        size
    }
}
