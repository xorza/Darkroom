//! Tests for Moffat profile fitting.

use super::*;
use crate::star_detection::centroid::lm_optimizer::LMConfig;

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_moffat_stamp(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    amplitude: f32,
    alpha: f32,
    beta: f32,
    background: f32,
) -> Vec<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
            let value = amplitude * (1.0 + r2 / (alpha * alpha)).powf(-beta);
            pixels[y * width + x] += value;
        }
    }
    pixels
}

#[test]
fn test_moffat_fit_centered_fixed_beta() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;

    let pixels = make_moffat_stamp(
        width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: true_beta,
        ..Default::default()
    };
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.x - true_cx).abs() < 0.1);
    assert!((result.y - true_cy).abs() < 0.1);
    assert!((result.alpha - true_alpha).abs() < 0.3);
}

#[test]
fn test_moffat_fit_subpixel_offset() {
    let width = 21;
    let height = 21;
    let true_cx = 10.3;
    let true_cy = 10.7;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;

    let pixels = make_moffat_stamp(
        width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: true_beta,
        ..Default::default()
    };
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.x - true_cx).abs() < 0.05);
    assert!((result.y - true_cy).abs() < 0.05);
}

#[test]
fn test_moffat_fit_with_beta() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 3.5;
    let true_bg = 0.1;

    let pixels = make_moffat_stamp(
        width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: true,
        fixed_beta: 3.0,
        lm: LMConfig {
            max_iterations: 100,
            ..Default::default()
        },
    };
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.x - true_cx).abs() < 0.1);
    assert!((result.y - true_cy).abs() < 0.1);
    assert!((result.beta - true_beta).abs() < 0.5);
}

#[test]
fn test_alpha_beta_fwhm_conversion() {
    let alpha = 2.0;
    let beta = 2.5;
    let fwhm = alpha_beta_to_fwhm(alpha, beta);
    let alpha_back = fwhm_beta_to_alpha(fwhm, beta);
    assert!((alpha_back - alpha).abs() < 1e-6);
    assert!((fwhm - 2.26).abs() < 0.1);
}

#[test]
fn test_moffat_fit_edge_position() {
    let width = 21;
    let height = 21;
    let pixels = vec![0.1f32; width * height];
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig::default();
    let result = fit_moffat_2d(&pixels_buf, 2.0, 10.0, 8, 0.1, &config);
    assert!(result.is_none());
}
