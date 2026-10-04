//! Gaussian convolution for matched filtering in star detection.
//!
//! Implements separable Gaussian convolution which is O(n×k) instead of O(n×k²)
//! where k is the kernel size. This is the key technique used by DAOFIND and
//! SExtractor to boost SNR for faint star detection.
//!
//! Uses SIMD acceleration when available (AVX2/SSE on `x86_64`, NEON on aarch64).

mod simd;

use std::f64::consts::FRAC_PI_2;
use std::mem;

use rayon::prelude::*;

use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::math::fwhm::fwhm_to_sigma;
use crate::math::pixel_gaussian::PixelGaussian;
use crate::math::pixel_quadrature::PixelQuadrature;
use crate::math::size2us::Size2us;
use crate::star_detection::config::fwhm_config::MatchedFilter;
use imaginarium::Buffer2;

/// Maximum deviation of `axis_ratio` from 1.0 to use the faster separable
/// (circular) kernel path instead of full 2D elliptical convolution.
const CIRCULAR_KERNEL_THRESHOLD: f32 = 0.01;

/// The Gauss–Legendre order of an elliptical kernel's pixel means. The error falls geometrically
/// with the order: order 8 errs by 2e-12 of the peak at σ 0.5, and 16 is under f64 rounding. The
/// kernel is built once per frame, so the order costs nothing that matters.
const ELLIPTICAL_KERNEL_ORDER: usize = 16;

/// The matched filter's kernel weights, kept from frame to frame: the row and column kernels of
/// a separable filter, or the 2D kernel in `rows`.
#[derive(Debug, Default)]
pub(crate) struct FilterKernels {
    rows: Vec<f32>,
    cols: Vec<f32>,
}

/// Apply matched filter convolution optimized for star detection, in place, with `temp` — of any
/// contents and `values`' size — for the intermediate pass, and `kernels` for the weights.
///
/// Convolves the residual — the image less its sky, so the convolution carries no pedestal — with a
/// Gaussian kernel matching the expected PSF as the pixels record it: each weight is the PSF's
/// mean over its pixel. The output is normalized by `sqrt(sum(K^2))`, which
/// keeps white noise at its σ; the detection plane measures the σ its output has anyway, which
/// holds for noise of any correlation.
///
/// SEP's matched filter (Barbary 2016) where the noise is uniform:
/// `SNR = conv(D, K) / (sigma * sqrt(sum(K^2)))`. Where σ varies, SEP weighs each pixel by `1/σ²`
/// inside the sums; this does not, and the threshold reads the local σ of the output instead.
///
/// Supports elliptical PSF shapes for stars elongated due to tracking errors,
/// field rotation, or optical aberrations. For circular PSFs, use `axis_ratio = 1.0`. An ellipse
/// whose axes lie along the pixel axes is a product of two 1D profiles, and so is its pixel mean,
/// so it filters in a row pass and a column pass. At any other angle the kernel is 2D: Geusebroek,
/// van den Boomgaard and Smeulders (2003) split a rotated Gaussian into an axis pass and a skewed
/// one, but the skewed pass interpolates between pixels, and the kernel is then no longer the
/// PSF's pixel mean.
pub(crate) fn matched_filter(
    values: &mut Buffer2<f32>,
    filter: MatchedFilter,
    temp: &mut Buffer2<f32>,
    kernels: &mut FilterKernels,
) {
    assert_eq!(values.width(), temp.width());
    assert_eq!(values.height(), temp.height());
    let MatchedFilter {
        fwhm,
        axis_ratio,
        angle,
    } = filter;
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    let sigma = fwhm_to_sigma(fwhm);

    // sqrt(sum(K²)) of the kernel the convolution used; an axis ratio this close to 1 takes the
    // separable circular kernel.
    let noise_norm = if (axis_ratio - 1.0).abs() < CIRCULAR_KERNEL_THRESHOLD {
        let radius = kernel_radius(sigma);
        separable_convolve(values, [sigma, sigma], radius, temp, kernels)
    } else if let Some(turns) = quarter_turns(angle) {
        // The 2D kernel's footprint, out to 3σ of the major axis both ways.
        let radius = kernel_radius(sigma);
        let minor = sigma * axis_ratio;
        let sigmas = if turns % 2 == 0 {
            [sigma, minor]
        } else {
            [minor, sigma]
        };
        separable_convolve(values, sigmas, radius, temp, kernels)
    } else {
        let norm = convolve_elliptical(values, sigma, axis_ratio, angle, temp, kernels);
        mem::swap(values, temp);
        norm
    };

    // After convolution the noise is the map's times sqrt(sum(K²)); dividing it out puts the
    // filtered image back on the map's scale.
    let inv_norm = 1.0 / noise_norm;
    values
        .pixels_mut()
        .par_chunks_mut(SAMPLES_PER_BLOCK)
        .for_each(|block| block.iter_mut().for_each(|px| *px *= inv_norm));
}

/// The quarter turns of `angle` when it is the f32 nearest a multiple of π/2, which is as near
/// to the pixel axes as a stored angle comes.
fn quarter_turns(angle: f32) -> Option<i64> {
    let turns = (f64::from(angle) / FRAC_PI_2).round();
    ((turns * FRAC_PI_2) as f32 == angle).then_some(turns as i64)
}

/// A kernel's half-width: out to `3σ`.
#[expect(
    clippy::cast_sign_loss,
    reason = "σ derives from a validated, positive FWHM"
)]
fn kernel_radius(sigma: f32) -> usize {
    (3.0 * sigma).ceil() as usize
}

/// Convolve `values` in place with the Gaussian of `[σx, σy]` along the axes, out to `radius`
/// both ways, using `temp` for the row pass.
///
/// Separable: the rows, then the columns, O(n×k) instead of O(n×k²). Both passes mirror at the
/// edges per axis, so they hold for a kernel wider than the image too. Returns `sqrt(sum(K²))` of
/// the equivalent normalized 2D kernel, `sqrt(Σkx²·Σky²)`: for a circular kernel, `Σk²` exactly.
fn separable_convolve(
    values: &mut Buffer2<f32>,
    [sigma_x, sigma_y]: [f32; 2],
    radius: usize,
    temp: &mut Buffer2<f32>,
    kernels: &mut FilterKernels,
) -> f32 {
    assert!(sigma_x > 0.0 && sigma_y > 0.0, "Sigma must be positive");
    assert_eq!(values.width(), temp.width());
    assert_eq!(values.height(), temp.height());

    fill_gaussian_kernel_1d(sigma_x, radius, &mut kernels.rows);
    fill_gaussian_kernel_1d(sigma_y, radius, &mut kernels.cols);
    // The column pass reads the row pass's output alone, so it writes back over the input.
    convolve_rows_parallel(values, temp, &kernels.rows);
    convolve_cols(temp, values, &kernels.cols);
    let squares = |kernel: &[f32]| kernel.iter().map(|&weight| weight * weight).sum::<f32>();
    (squares(&kernels.rows) * squares(&kernels.cols)).sqrt()
}

/// Apply elliptical Gaussian convolution to an image, with `kernels` for the 2D weights.
///
/// Full 2D convolution, O(n×k²), for an ellipse at an angle to the pixel axes. Returns
/// `sqrt(sum(K²))` for the normalized 2D kernel.
fn convolve_elliptical(
    pixels: &Buffer2<f32>,
    sigma: f32,
    axis_ratio: f32,
    angle: f32,
    output: &mut Buffer2<f32>,
    kernels: &mut FilterKernels,
) -> f32 {
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());

    let size = fill_elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle, &mut kernels.rows);
    convolve_2d(pixels, &kernels.rows, size, output);
    kernels
        .rows
        .iter()
        .map(|&weight| weight * weight)
        .sum::<f32>()
        .sqrt()
}

fn convolve_2d(pixels: &Buffer2<f32>, weights: &[f32], size: usize, output: &mut Buffer2<f32>) {
    let width = pixels.width();
    let height = pixels.height();
    let kernel = simd::Kernel2d::new(weights, size);

    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out_row)| {
            simd::convolve_2d_row(
                pixels.pixels(),
                out_row,
                Size2us::new(width, height),
                y,
                kernel,
            );
        });
}

/// Into `kernel`, the 1D Gaussian of `sigma` out to `radius`, each weight the profile's mean over
/// its pixel, normalized to sum to 1.
fn fill_gaussian_kernel_1d(sigma: f32, radius: usize, kernel: &mut Vec<f32>) {
    assert!(sigma > 0.0, "Sigma must be positive");
    let gaussian = PixelGaussian {
        sigma: f64::from(sigma),
    };
    let mean = |i: usize| gaussian.mean_at(i as f64 - radius as f64);
    let sum: f64 = (0..=2 * radius).map(mean).sum();
    kernel.clear();
    kernel.extend((0..=2 * radius).map(|i| (mean(i) / sum) as f32));
}

/// Convolve all rows in parallel, vectors within each row.
fn convolve_rows_parallel(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    output
        .pixels_mut()
        .par_chunks_mut(input.width())
        .enumerate()
        .for_each(|(y, out_row)| {
            simd::convolve_row(input.row(y), out_row, kernel);
        });
}

/// Convolve all columns: rayon-parallel over output rows, vectors across the columns within each
/// row.
fn convolve_cols(input: &Buffer2<f32>, output: &mut Buffer2<f32>, kernel: &[f32]) {
    let size = Size2us::new(input.width(), input.height());
    simd::convolve_cols_direct(input.pixels(), output.pixels_mut(), size, kernel);
}

/// Into `weights`, row by row, the 2D elliptical Gaussian kernel out to `3σ` of the major axis,
/// each weight the profile's mean over its pixel, normalized to sum to 1; returns its side.
fn fill_elliptical_gaussian_kernel_2d(
    sigma: f32,
    axis_ratio: f32,
    angle: f32,
    weights: &mut Vec<f32>,
) -> usize {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    let radius = kernel_radius(sigma);
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

    let rule = PixelQuadrature::gauss_legendre(ELLIPTICAL_KERNEL_ORDER);
    let mean = |index: usize| {
        let x = (index % size) as f64 - radius as f64;
        let y = (index / size) as f64 - radius as f64;
        rule.integrate(x, y, profile)
    };
    let sum: f64 = (0..size * size).map(mean).sum();
    weights.clear();
    weights.extend((0..size * size).map(|index| (mean(index) / sum) as f32));
    size
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// The 1D kernel of `sigma` out to `3σ`.
    pub(crate) fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
        let mut kernel = Vec::new();
        fill_gaussian_kernel_1d(sigma, kernel_radius(sigma), &mut kernel);
        kernel
    }

    /// The circular separable convolution on fresh kernels.
    pub(crate) fn gaussian_convolve(
        values: &mut Buffer2<f32>,
        sigma: f32,
        temp: &mut Buffer2<f32>,
    ) -> f32 {
        separable_convolve(
            values,
            [sigma, sigma],
            kernel_radius(sigma),
            temp,
            &mut FilterKernels::default(),
        )
    }

    /// A 2D kernel's weights, row by row, and its side.
    #[derive(Debug)]
    pub(crate) struct GaussianKernel2d {
        pub(crate) weights: Vec<f32>,
        pub(crate) size: usize,
    }

    pub(crate) fn elliptical_gaussian_kernel_2d(
        sigma: f32,
        axis_ratio: f32,
        angle: f32,
    ) -> GaussianKernel2d {
        let mut weights = Vec::new();
        let size = fill_elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle, &mut weights);
        GaussianKernel2d { weights, size }
    }

    /// The full 2D convolution on fresh kernels, at any angle.
    pub(crate) fn elliptical_gaussian_convolve(
        pixels: &Buffer2<f32>,
        sigma: f32,
        axis_ratio: f32,
        angle: f32,
        output: &mut Buffer2<f32>,
    ) -> f32 {
        convolve_elliptical(
            pixels,
            sigma,
            axis_ratio,
            angle,
            output,
            &mut FilterKernels::default(),
        )
    }

    /// [`matched_filter`] on fresh kernels.
    pub(crate) fn matched_filter_fresh(
        values: &mut Buffer2<f32>,
        filter: MatchedFilter,
        temp: &mut Buffer2<f32>,
    ) {
        matched_filter(values, filter, temp, &mut FilterKernels::default());
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
