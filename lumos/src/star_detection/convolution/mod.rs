//! Gaussian convolution for matched filtering in star detection.
//!
//! Implements separable Gaussian convolution which is O(n×k) instead of O(n×k²)
//! where k is the kernel size. This is the key technique used by DAOFIND and
//! SExtractor to boost SNR for faint star detection.
//!
//! Uses SIMD acceleration when available (AVX2/SSE on `x86_64`, NEON on aarch64).

mod simd;

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

#[derive(Debug)]
struct GaussianKernel2d {
    weights: Vec<f32>,
    size: usize,
}

/// Apply matched filter convolution optimized for star detection, in place, with `temp` — of any
/// contents and `values`' size — for the intermediate pass.
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
/// field rotation, or optical aberrations. For circular PSFs, use `axis_ratio = 1.0`.
pub(crate) fn matched_filter(
    values: &mut Buffer2<f32>,
    filter: MatchedFilter,
    temp: &mut Buffer2<f32>,
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
        gaussian_convolve(values, sigma, temp)
    } else {
        let norm = elliptical_gaussian_convolve(values, sigma, axis_ratio, angle, temp);
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

/// Convolve `values` in place with a circular Gaussian of `sigma`, using `temp` for the row pass.
///
/// Separable: the rows, then the columns, O(n×k) instead of O(n×k²). Both passes mirror at the
/// edges per axis, so they hold for a kernel wider than the image too. Returns `sqrt(sum(K²))` of
/// the equivalent normalized 2D kernel.
fn gaussian_convolve(values: &mut Buffer2<f32>, sigma: f32, temp: &mut Buffer2<f32>) -> f32 {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert_eq!(values.width(), temp.width());
    assert_eq!(values.height(), temp.height());

    let kernel = gaussian_kernel_1d(sigma);
    // The column pass reads the row pass's output alone, so it writes back over the input.
    convolve_rows_parallel(values, temp, &kernel);
    convolve_cols(temp, values, &kernel);
    kernel.iter().map(|&weight| weight * weight).sum()
}

/// Apply elliptical Gaussian convolution to an image.
///
/// Unlike separable convolution for circular Gaussians, elliptical Gaussians
/// require full 2D convolution which is O(n×k²). This is used when the PSF
/// is known to be non-circular.
/// Returns `sqrt(sum(K²))` for the normalized 2D kernel.
fn elliptical_gaussian_convolve(
    pixels: &Buffer2<f32>,
    sigma: f32,
    axis_ratio: f32,
    angle: f32,
    output: &mut Buffer2<f32>,
) -> f32 {
    assert_eq!(pixels.width(), output.width());
    assert_eq!(pixels.height(), output.height());

    let kernel = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
    convolve_2d(pixels, &kernel, output);
    kernel
        .weights
        .iter()
        .map(|&weight| weight * weight)
        .sum::<f32>()
        .sqrt()
}

fn convolve_2d(pixels: &Buffer2<f32>, kernel: &GaussianKernel2d, output: &mut Buffer2<f32>) {
    let width = pixels.width();
    let height = pixels.height();
    let kernel = simd::Kernel2d::new(&kernel.weights, kernel.size);

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

/// The 1D Gaussian kernel of `sigma` out to `3σ`, each weight the profile's mean over its pixel,
/// normalized to sum to 1.
#[expect(
    clippy::cast_sign_loss,
    reason = "σ derives from a validated, positive FWHM"
)]
fn gaussian_kernel_1d(sigma: f32) -> Vec<f32> {
    assert!(sigma > 0.0, "Sigma must be positive");

    let radius = (3.0 * sigma).ceil() as usize;
    let gaussian = PixelGaussian {
        sigma: f64::from(sigma),
    };
    let means: Vec<f64> = (0..=2 * radius)
        .map(|i| gaussian.mean_at(i as f64 - radius as f64))
        .collect();
    let sum: f64 = means.iter().sum();
    means.iter().map(|&mean| (mean / sum) as f32).collect()
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

/// The 2D elliptical Gaussian kernel out to `3σ` of the major axis, each weight the profile's mean
/// over its pixel, normalized to sum to 1.
#[expect(
    clippy::cast_sign_loss,
    reason = "σ derives from a validated, positive FWHM"
)]
fn elliptical_gaussian_kernel_2d(sigma: f32, axis_ratio: f32, angle: f32) -> GaussianKernel2d {
    assert!(sigma > 0.0, "Sigma must be positive");
    assert!(
        axis_ratio > 0.0 && axis_ratio <= 1.0,
        "Axis ratio must be in (0, 1]"
    );

    let radius = (3.0 * sigma).ceil() as usize;
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
    let means: Vec<f64> = (0..size * size)
        .map(|index| {
            let x = (index % size) as f64 - radius as f64;
            let y = (index / size) as f64 - radius as f64;
            rule.integrate(x, y, profile)
        })
        .collect();
    let sum: f64 = means.iter().sum();
    GaussianKernel2d {
        weights: means.iter().map(|&mean| (mean / sum) as f32).collect(),
        size,
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
