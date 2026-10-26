//! [`FilterKernels`]: the matched filter, over kernel weights it keeps from frame to frame.

use crate::math::fwhm;
use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};
use crate::star_detection::convolution;
use crate::star_detection::convolution::simd::{
    Convolve2dRow, ConvolveColsRow, ConvolveRow, Kernel2d, RowRing,
};
use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;
use common::Size2us;
use simd::Kernel;

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

/// A matched filter's kernel over the weights a [`FilterKernels`] holds, run a row at a time: its
/// shape, and the factor that puts the filtered image back on its input's noise scale.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MatchedKernel<'a> {
    shape: MatchedShape<'a>,
    inv_norm: f32,
}

/// The weights of a [`MatchedKernel`]: a row pass and a column pass, or one 2D kernel.
#[derive(Debug, Clone, Copy)]
enum MatchedShape<'a> {
    Separable { rows: &'a [f32], cols: &'a [f32] },
    Elliptical(Kernel2d<'a>),
}

impl FilterKernels {
    /// The matched filter's kernel for `filter`, its weights filled into this set.
    ///
    /// It convolves the residual — the image less its sky, so the convolution carries no pedestal
    /// — with a Gaussian kernel matching the expected PSF as the pixels record it: each weight is
    /// the PSF's mean over its pixel. The output is normalized by `sqrt(sum(K^2))`, which keeps
    /// white noise at its σ; the detection plane measures the σ its output has anyway, which holds
    /// for noise of any correlation.
    ///
    /// SEP's matched filter (Barbary 2016) where the noise is uniform:
    /// `SNR = conv(D, K) / (sigma * sqrt(sum(K^2)))`. Where σ varies, SEP weighs each pixel by
    /// `1/σ²` inside the sums; this does not, and the threshold reads the local σ of the output
    /// instead.
    ///
    /// Supports elliptical PSF shapes for stars elongated due to tracking errors, field rotation,
    /// or optical aberrations. An ellipse whose axes lie along the pixel axes is a product of two
    /// 1D profiles, and so is its pixel mean, so it filters in a row pass and a column pass, out to
    /// `3σ` of the wider both ways. At any other angle the kernel is 2D: Geusebroek, van den
    /// Boomgaard and Smeulders (2003) split a rotated Gaussian into an axis pass and a skewed one,
    /// but the skewed pass interpolates between pixels, and the kernel is then no longer the PSF's
    /// pixel mean.
    pub(crate) fn matched(&mut self, filter: MatchedFilter) -> MatchedKernel<'_> {
        debug_assert!(
            filter.axis_ratio > 0.0 && filter.axis_ratio <= 1.0,
            "Axis ratio must be in (0, 1]"
        );
        let squares = |kernel: &[f32]| kernel.iter().map(|&weight| weight * weight).sum::<f32>();
        // sqrt(sum(K²)) of the kernel: for a separable one, of the equivalent 2D kernel,
        // `sqrt(Σkx²·Σky²)`, which for a circular kernel is `Σk²` exactly.
        let (shape, noise_norm) = match KernelShape::of(filter) {
            KernelShape::Separable([sigma_x, sigma_y]) => {
                debug_assert!(sigma_x > 0.0 && sigma_y > 0.0, "Sigma must be positive");
                let radius = convolution::kernel_radius(sigma_x.max(sigma_y));
                convolution::fill_gaussian_kernel_1d(sigma_x, radius, &mut self.rows);
                convolution::fill_gaussian_kernel_1d(sigma_y, radius, &mut self.cols);
                let norm = (squares(&self.rows) * squares(&self.cols)).sqrt();
                (
                    MatchedShape::Separable {
                        rows: &self.rows,
                        cols: &self.cols,
                    },
                    norm,
                )
            }
            KernelShape::Elliptical(ellipse) => {
                let size = ellipse.fill_kernel(&mut self.rows);
                let norm = squares(&self.rows).sqrt();
                (
                    MatchedShape::Elliptical(Kernel2d::new(&self.rows, size)),
                    norm,
                )
            }
        };
        MatchedKernel {
            shape,
            inv_norm: 1.0 / noise_norm,
        }
    }
}

impl MatchedKernel<'_> {
    /// The rows above and below an output row that its taps read.
    pub(crate) const fn radius(self) -> usize {
        match self.shape {
            MatchedShape::Separable { cols, .. } => cols.len() / 2,
            MatchedShape::Elliptical(kernel) => kernel.radius(),
        }
    }

    /// Into `ring_row`, what the column taps read of an input row: its row pass for a separable
    /// kernel, the row itself for a 2D one.
    pub(crate) fn ring_row(self, input: &[f32], ring_row: &mut [f32]) {
        match self.shape {
            MatchedShape::Separable { rows, .. } => ConvolveRow {
                input,
                output: ring_row,
                kernel: rows,
            }
            .dispatch(),
            MatchedShape::Elliptical(_) => ring_row.copy_from_slice(input),
        }
    }

    /// Output row `y` of an image of `size` into `output`, from `ring`, whole rows of
    /// [`Self::ring_row`]s, image row `n` in slot `n` modulo its slots, holding every row the taps
    /// read: mirrored at the image's edges, and scaled back to the input's noise.
    pub(crate) fn output_row(self, ring: &[f32], size: Size2us, y: usize, output: &mut [f32]) {
        debug_assert_eq!(output.len(), size.width, "one output row");
        let input = RowRing::new(ring, size);
        match self.shape {
            MatchedShape::Separable { cols, .. } => ConvolveColsRow {
                input,
                out_row: output,
                y,
                kernel: cols,
            }
            .dispatch(),
            MatchedShape::Elliptical(kernel) => Convolve2dRow {
                input,
                output_row: output,
                y,
                kernel,
            }
            .dispatch(),
        }
        for sample in output {
            *sample *= self.inv_norm;
        }
    }
}

impl EllipticalGaussian {
    /// The narrowest σ the ellipse has along either pixel axis at a fixed offset along the other:
    /// `1/√P` for the larger diagonal entry `P` of its inverse covariance. Near `σ·axis_ratio` for
    /// a thin ellipse off the axes, it is what the pixel quadrature has to resolve.
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
        let mean = |idx: usize| {
            let x = (idx % size) as f64 - radius as f64;
            let y = (idx / size) as f64 - radius as f64;
            let mut sum = 0.0;
            for row in 0..subdivision {
                let cell_y = y - 0.5 + (row as f64 + 0.5) * cell;
                for col in 0..subdivision {
                    let cell_x = x - 0.5 + (col as f64 + 0.5) * cell;
                    sum += rule.integrate(0.0, 0.0, |u, v| {
                        profile(cell_x + u * cell, cell_y + v * cell)
                    });
                }
            }
            sum * cell * cell
        };
        let sum: f64 = (0..size * size).map(mean).sum();
        weights.clear();
        weights.extend((0..size * size).map(|idx| (mean(idx) / sum) as f32));
        size
    }
}
