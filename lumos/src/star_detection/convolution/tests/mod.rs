//! Tests for Gaussian convolution.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

mod matched_filter;

use crate::internals::prelude::*;
use crate::internals::synthetic::patterns;
use std::f32::consts::FRAC_PI_2;

use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::math::fwhm::sigma_to_fwhm;
use crate::star_detection::convolution::internals::*;
use crate::star_detection::convolution::*;

/// `pixels` less `background`: the residual the matched filter takes.
fn less(pixels: &Buffer2<f32>, background: &Buffer2<f32>) -> Buffer2<f32> {
    let mut residual = pixels.clone();
    for (value, &sky) in residual.iter_mut().zip(background.iter()) {
        *value -= sky;
    }
    residual
}

#[test]
fn gaussian_kernel_1d_normalization() {
    // Divided by its own f32 sum: summing n weights ≤ 1 again rounds by at most n·ε.
    for sigma in [0.5, 1.0, 2.0, 3.0, 5.0] {
        let kernel = gaussian_kernel_1d(sigma);
        let sum: f32 = kernel.iter().sum();
        assert!(
            (sum - 1.0).abs() <= kernel.len() as f32 * f32::EPSILON,
            "Kernel should sum to 1.0, got {sum} for sigma={sigma}"
        );
    }
}

#[test]
fn gaussian_kernel_1d_symmetry() {
    // ±x square to the same value, so the two halves are equal bit for bit.
    let kernel = gaussian_kernel_1d(2.0);
    let n = kernel.len();
    for i in 0..n / 2 {
        assert_eq!(kernel[i], kernel[n - 1 - i]);
    }
}

#[test]
fn gaussian_kernel_1d_peak_at_center() {
    let kernel = gaussian_kernel_1d(2.0);
    let center = kernel.len() / 2;
    for (i, &v) in kernel.iter().enumerate() {
        if i != center {
            assert!(v < kernel[center], "Center should have maximum value");
        }
    }
}

#[test]
fn gaussian_kernel_1d_size() {
    // Kernel radius should be ceil(3 * sigma)
    let sigma = 2.0;
    let kernel = gaussian_kernel_1d(sigma);
    let expected_radius = (3.0 * sigma).ceil() as usize;
    let expected_size = 2 * expected_radius + 1;
    assert_eq!(kernel.len(), expected_size);
}

#[test]
fn gaussian_kernel_1d_small_sigma() {
    let kernel = gaussian_kernel_1d(0.5);
    // For sigma=0.5, radius=2, size=5
    assert_eq!(kernel.len(), 5);
    assert!(kernel[2] > 0.5, "Center should dominate for small sigma");
}

#[test]
#[should_panic(expected = "Sigma must be positive")]
fn gaussian_kernel_1d_zero_sigma_panics() {
    gaussian_kernel_1d(0.0);
}

#[test]
#[should_panic(expected = "Sigma must be positive")]
fn gaussian_kernel_1d_negative_sigma_panics() {
    gaussian_kernel_1d(-1.0);
}

#[test]
fn gaussian_convolve_keeps_a_uniform_image() {
    // Each pass sums the n weights times one value, so a uniform v comes back as v·Σk twice, off
    // by at most 2n·ε relative. 8×8 at σ = 2 has a kernel (13 taps) wider than the image, and
    // 512×512 runs the vector paths across long rows.
    for (side, sigma) in [(8, 2.0), (32, 2.0), (512, 3.0)] {
        let pixels = Buffer2::new_filled(side, side, 0.5f32);
        let mut result = Buffer2::new_default(side, side);
        let mut temp = Buffer2::new_default(side, side);
        {
            result.pixels_mut().copy_from_slice(pixels.pixels());
            gaussian_convolve(&mut result, sigma, &mut temp)
        };

        let bound = 0.5 * 2.0 * gaussian_kernel_1d(sigma).len() as f32 * f32::EPSILON;
        for (i, v) in result.iter().enumerate() {
            assert!(
                (v - 0.5).abs() <= bound,
                "{side}×{side}, σ = {sigma}: pixel {i} is {v}"
            );
        }
    }
}

#[test]
fn gaussian_convolve_preserves_total_flux() {
    // A centred point source spreads into K[x]·K[y], whose sum is (Σk)² = 1 up to the kernel's
    // own n·ε twice and the n² products summed here: (n² + 2n)·ε.
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(32, 32)] = 1.0;

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    {
        result.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(&mut result, 2.0, &mut temp)
    };

    let n = gaussian_kernel_1d(2.0).len() as f32;
    let output_sum: f32 = result.iter().sum();
    assert!(
        (output_sum - 1.0).abs() <= (n * n + 2.0 * n) * f32::EPSILON,
        "Total flux should be preserved: output={output_sum}"
    );
}

#[test]
fn gaussian_convolve_spreads_point_source() {
    // A unit delta: each pass has one nonzero term, so every output is the single product
    // K[x]·K[y], bit for bit — the peak K[c]², its neighbour K[c]·K[c+1].
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(32, 32)] = 1.0;
    let mut temp = Buffer2::new_default(width, height);

    for sigma in [1.0f32, 2.0, 3.0] {
        let mut result = Buffer2::new_default(width, height);
        {
            result.pixels_mut().copy_from_slice(pixels.pixels());
            gaussian_convolve(&mut result, sigma, &mut temp)
        };

        let kernel = gaussian_kernel_1d(sigma);
        let c = kernel.len() / 2;
        assert_eq!(result[(32, 32)], kernel[c] * kernel[c], "σ = {sigma}: peak");
        assert_eq!(
            result[(33, 32)],
            kernel[c + 1] * kernel[c],
            "σ = {sigma}: neighbour"
        );
    }
}

#[test]
fn gaussian_convolve_symmetry() {
    // Every output of a delta is a product of two symmetric kernel weights, so reflections agree
    // bit for bit.
    let width = 33;
    let height = 33;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(16, 16)] = 1.0;

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    {
        result.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(&mut result, 2.0, &mut temp)
    };

    for dy in 1..8 {
        for dx in 1..8 {
            let v = result[(16 + dx, 16 + dy)];
            assert_eq!(result[(16 + dx, 16 - dy)], v);
            assert_eq!(result[(16 - dx, 16 + dy)], v);
            assert_eq!(result[(16 - dx, 16 - dy)], v);
        }
    }
}

#[test]
fn gaussian_convolve_edge_handling() {
    // A delta at (2, 2) with σ = 1.5 (radius 5): the mirror reflects tap −2 onto pixel 2, so the
    // row pass at x = 2 takes K[1] (the reflection) then K[5] (the pixel itself), and the column
    // pass does the same to that sum. In that order the result is exact.
    let width = 16;
    let height = 16;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(2, 2)] = 1.0;

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    {
        result.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(&mut result, 1.5, &mut temp)
    };

    let kernel = gaussian_kernel_1d(1.5);
    assert_eq!(kernel.len(), 11);
    let row = kernel[1] + kernel[5];
    assert_eq!(result[(2, 2)], row * kernel[1] + row * kernel[5]);
}

#[test]
fn gaussian_convolve_non_square_image() {
    // 64×32 non-square image with point source at (32,16)
    let width = 64;
    let height = 32;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(32, 16)] = 1.0;

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    {
        result.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(&mut result, 2.0, &mut temp)
    };

    assert_eq!(result.len(), width * height);

    // Peak should match kernel center^2
    let kernel = gaussian_kernel_1d(2.0);
    let center = kernel[kernel.len() / 2];
    let peak = result.row(16)[32];
    assert!(
        (peak - center * center).abs() < 1e-5,
        "Non-square peak {} should match kernel center^2 = {}",
        peak,
        center * center
    );
}

#[test]
fn matched_filter_of_a_zero_residual_is_zero() {
    let width = 32;
    let height = 32;
    let residual = Buffer2::new_filled(width, height, 0.0f32);

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    result.pixels_mut().copy_from_slice(residual.pixels());
    matched_filter_fresh(
        &mut result,
        MatchedFilter {
            fwhm: 3.0,
            axis_ratio: 1.0,
            angle: 0.0,
        },
        &mut temp,
    );

    // Every tap multiplies a zero: the sums are exactly zero, scaled or not.
    assert!(result.iter().all(|&v| v == 0.0));
}

#[test]
fn matched_filter_detects_star() {
    // A lone residual of 0.5 − 0.1 at (16, 16): the separable passes give 0.4·K[c]·K[c], and the
    // normalization divides by √(ΣK²) of the 2D kernel, Σk² of the 1D one. Each step has one
    // nonzero term, so the peak is that product exactly, and the image maximum.
    let width = 32;
    let height = 32;
    let background = Buffer2::new_filled(width, height, 0.1f32);
    let mut pixels = Buffer2::new_filled(width, height, 0.1f32);
    pixels[(16, 16)] = 0.5;

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    result
        .pixels_mut()
        .copy_from_slice(less(&pixels, &background).pixels());
    matched_filter_fresh(
        &mut result,
        MatchedFilter {
            fwhm: 3.0,
            axis_ratio: 1.0,
            angle: 0.0,
        },
        &mut temp,
    );

    let kernel = gaussian_kernel_1d(fwhm_to_sigma(3.0));
    let c = kernel.len() / 2;
    let norm: f32 = kernel.iter().map(|&k| k * k).sum();
    let expected = (0.5f32 - 0.1) * kernel[c] * kernel[c] * (1.0 / norm);
    assert_eq!(result[(16, 16)], expected);
    let max_val = result.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(max_val, expected);
}

#[test]
fn matched_filter_peaks_at_the_star() {
    // A 0.3 star (σ 2) under Gaussian noise of 0.03: the filtered maximum is at its centre.
    let width = 64;
    let height = 64;
    let (cx, cy) = (32, 32);
    let sigma = 2.0;
    let mut residual = Buffer2::new_filled(width, height, 0.0f32);
    SyntheticStar::new(
        Vec2::new(cx as f32, cy as f32),
        0.3,
        StarProfile::Gaussian { sigma },
    )
    .add_to(&mut residual);
    patterns::add_gaussian_noise(residual.pixels_mut(), 0.03, 7);

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    result.pixels_mut().copy_from_slice(residual.pixels());
    matched_filter_fresh(
        &mut result,
        MatchedFilter {
            fwhm: sigma_to_fwhm(sigma),
            axis_ratio: 1.0,
            angle: 0.0,
        },
        &mut temp,
    );

    let max_val = result.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let max_idx = result.iter().position(|&v| v == max_val).unwrap();
    assert_eq!((max_idx % width, max_idx / width), (cx, cy));
}

#[test]
fn matched_filter_preserves_negative_residuals() {
    let width = 16;
    let height = 16;
    let background = Buffer2::new_filled(width, height, 0.5f32);
    let pixels = Buffer2::new_filled(width, height, 0.3f32); // Below background

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);
    result
        .pixels_mut()
        .copy_from_slice(less(&pixels, &background).pixels());
    matched_filter_fresh(
        &mut result,
        MatchedFilter {
            fwhm: 2.0,
            axis_ratio: 1.0,
            angle: 0.0,
        },
        &mut temp,
    );

    // Negative residuals are preserved for correct noise statistics.
    // Uniform below-background input should produce negative convolved output.
    for &v in &result {
        assert!(
            v < 0.0,
            "Below-background pixels should produce negative output"
        );
    }
}

#[test]
fn matched_filter_noise_normalization() {
    // After noise normalization, the standard deviation of the output on a
    // pure-noise image should approximately match the input noise level.
    // This verifies the sqrt(sum(K^2)) normalization is correct.
    let width = 256;
    let height = 256;
    let bg_level = 1000.0f32;
    let noise_sigma = 10.0f32;

    let mut pixels = Buffer2::new_filled(width, height, bg_level);
    patterns::add_gaussian_noise(pixels.pixels_mut(), noise_sigma, 42);

    let background = Buffer2::new_filled(width, height, bg_level);

    let mut result = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);

    for axis_ratio in [1.0, 0.7] {
        result
            .pixels_mut()
            .copy_from_slice(less(&pixels, &background).pixels());
        matched_filter_fresh(
            &mut result,
            MatchedFilter {
                fwhm: 4.0,
                axis_ratio,
                angle: 0.5,
            },
            &mut temp,
        );

        // Exclude the border region affected by mirror sampling.
        let margin = 10;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut count = 0usize;
        for y in margin..height - margin {
            for x in margin..width - margin {
                let v = f64::from(result.row(y)[x]);
                sum += v;
                sum_sq += v * v;
                count += 1;
            }
        }
        let mean = sum / count as f64;
        let variance = sum_sq / count as f64 - mean * mean;
        let output_sigma = variance.sqrt();

        // √(ΣK²) normalization makes white noise come out at its own σ, exactly in
        // expectation. The 236² samples are correlated over the kernel: about 4π·σ_k² ≈ 36 px
        // per independent one at σ_k = 1.70, so ~1500 of them, and the sample σ scatters by
        // 1/√(2·1500) ≈ 1.8%. 10% is beyond 5σ of that (measured: 0.2% and 0.3%).
        let ratio = output_sigma / f64::from(noise_sigma);
        assert!(
            (ratio - 1.0).abs() < 0.1,
            "axis_ratio={axis_ratio}: output noise should match input noise after normalization; \
             ratio={ratio:.3}, output_sigma={output_sigma:.2}, input_sigma={noise_sigma}"
        );
    }
}

#[test]
fn separable_matches_outer_product_2d() {
    // The separable passes against one 2D pass with the outer-product kernel, both mirroring per
    // axis: equal algebraically. On 16×16 the σ = 1.5 kernel fits (radius 5); on 8×8 the σ = 3
    // kernel (radius 9) is wider than the image, the case that once took a separate direct path.
    // A 2D tap sum adds n² products of values ≤ 1 against the separable 2n, so the two agree to
    // n²·ε of f32.
    for (side, sigma) in [(16, 1.5f32), (8, 3.0)] {
        let mut pixels = Buffer2::new_filled(side, side, 0.0f32);
        for (i, p) in pixels.iter_mut().enumerate() {
            *p = ((i * 7 + 3) % 100) as f32 / 100.0;
        }

        let mut result_sep = Buffer2::new_default(side, side);
        let mut result_2d = Buffer2::new_default(side, side);
        let mut temp = Buffer2::new_default(side, side);
        {
            result_sep.pixels_mut().copy_from_slice(pixels.pixels());
            gaussian_convolve(&mut result_sep, sigma, &mut temp)
        };
        let kernel = gaussian_kernel_1d(sigma);
        let size = kernel.len();
        let weights: Vec<f32> = kernel
            .iter()
            .flat_map(|&ky| kernel.iter().map(move |&kx| ky * kx))
            .collect();
        convolve_2d(&pixels, &weights, size, &mut result_2d);

        let tolerance = (size * size) as f32 * f32::EPSILON;
        for (i, (&a, &b)) in result_sep.iter().zip(result_2d.iter()).enumerate() {
            assert!(
                (a - b).abs() < tolerance,
                "{side}x{side}, σ = {sigma}: separable and 2D differ at {i}: {a} vs {b}"
            );
        }
    }
}

/// An ellipse along the pixel axes filters in two passes as its 2D kernel does, and with the same
/// `sqrt(ΣK²)`: at no turn, a half turn, and a quarter turn each way, which sets the major axis on
/// the columns. The 2D weights are the quadrature's pixel means of the rotated profile, the 1D ones
/// exact, both to well under f32's rounding, and the f32 nearest π/2 turns the 2D kernel 4e-8 off
/// the axis, which moves a weight by under 1e-6 of itself; a 2D tap sum adds n² products of values
/// ≤ 1, so the two agree to n²·ε. A quarter turn that took the major axis on the rows would differ
/// by far more.
#[test]
fn an_axis_aligned_ellipse_filters_separably_as_its_2d_kernel() {
    let (side, sigma, axis_ratio) = (24, 1.5f32, 0.6f32);
    let mut pixels = Buffer2::new_filled(side, side, 0.0f32);
    for (i, p) in pixels.iter_mut().enumerate() {
        *p = ((i * 7 + 3) % 100) as f32 / 100.0;
    }
    let radius = kernel_radius(sigma);
    let size = 2 * radius + 1;
    let tolerance = (size * size) as f32 * f32::EPSILON;
    for (angle, sigmas) in [
        (0.0f32, [sigma, sigma * axis_ratio]),
        (std::f32::consts::PI, [sigma, sigma * axis_ratio]),
        (FRAC_PI_2, [sigma * axis_ratio, sigma]),
        (-FRAC_PI_2, [sigma * axis_ratio, sigma]),
    ] {
        let mut separable = pixels.clone();
        let mut temp = Buffer2::new_default(side, side);
        let separable_norm = separable_convolve(
            &mut separable,
            sigmas,
            radius,
            &mut temp,
            &mut FilterKernels::default(),
        );
        let mut full = Buffer2::new_default(side, side);
        let full_norm = elliptical_gaussian_convolve(&pixels, sigma, axis_ratio, angle, &mut full);
        assert!(
            (separable_norm - full_norm).abs() <= tolerance * full_norm,
            "angle {angle}: sqrt(ΣK²) {separable_norm} vs {full_norm}"
        );
        for (i, (&a, &b)) in separable.iter().zip(full.iter()).enumerate() {
            assert!(
                (a - b).abs() < tolerance,
                "angle {angle} at {i}: {a} vs {b}"
            );
        }
        if angle == FRAC_PI_2 {
            let mut swapped = pixels.clone();
            separable_convolve(
                &mut swapped,
                [sigma, sigma * axis_ratio],
                radius,
                &mut temp,
                &mut FilterKernels::default(),
            );
            let largest = swapped
                .iter()
                .zip(full.iter())
                .map(|(&a, &b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(
                largest > 100.0 * tolerance,
                "the axes would not matter: {largest}"
            );
        }
    }
}

/// The quarter turns of an angle stored as the f32 nearest a multiple of π/2, and none for an
/// angle an ulp away from one, or between.
#[test]
fn quarter_turns_are_the_angles_nearest_the_axes() {
    use std::f32::consts::PI;
    for (angle, turns) in [
        (0.0f32, Some(0)),
        (FRAC_PI_2, Some(1)),
        (-FRAC_PI_2, Some(-1)),
        (PI, Some(2)),
        (3.0 * FRAC_PI_2, Some(3)),
        (FRAC_PI_2.next_up(), None),
        (0.0f32.next_up(), None),
        (0.3, None),
    ] {
        assert_eq!(quarter_turns(angle), turns, "{angle}");
    }
}

#[test]
fn elliptical_kernel_normalization() {
    // Divided by its own f32 sum of n² weights: summing them again rounds by at most n²·ε.
    for sigma in [1.0, 2.0, 3.0] {
        for axis_ratio in [0.3, 0.5, 0.7, 1.0] {
            for angle in [0.0, 0.5, 1.0, 1.57] {
                let kernel = elliptical_gaussian_kernel_2d(sigma, axis_ratio, angle);
                let sum: f32 = kernel.weights.iter().sum();
                assert!(
                    (sum - 1.0).abs() <= kernel.weights.len() as f32 * f32::EPSILON,
                    "Elliptical kernel should sum to 1.0, got {sum} for sigma={sigma}, axis_ratio={axis_ratio}, angle={angle}"
                );
            }
        }
    }
}

#[test]
fn elliptical_kernel_symmetry_at_zero_angle() {
    // At angle 0 the rotation is x·1 + y·0 and −x·0 + y·1, exactly x and y, and the weight reads
    // only their squares: the four quadrants agree bit for bit.
    let kernel = elliptical_gaussian_kernel_2d(2.0, 0.5, 0.0);
    let center = kernel.size / 2;
    let at = |x: usize, y: usize| kernel.weights[y * kernel.size + x];
    for dy in 0..=center {
        for dx in 0..=center {
            let v = at(center + dx, center + dy);
            assert_eq!(at(center + dx, center - dy), v);
            assert_eq!(at(center - dx, center + dy), v);
            assert_eq!(at(center - dx, center - dy), v);
        }
    }
}

#[test]
fn elliptical_kernel_elongation() {
    // With axis_ratio < 1, kernel should be elongated along major axis
    let kernel = elliptical_gaussian_kernel_2d(2.0, 0.3, 0.0);
    let center = kernel.size / 2;

    // At angle=0, major axis is horizontal (x), minor axis is vertical (y)
    // Check that horizontal extent > vertical extent at same distance from center
    let dist = 2;
    let horizontal_val = kernel.weights[center * kernel.size + (center + dist)];
    let vertical_val = kernel.weights[(center + dist) * kernel.size + center];

    assert!(
        horizontal_val > vertical_val,
        "Horizontal extent should be larger than vertical for axis_ratio < 1 at angle=0"
    );
}

#[test]
fn elliptical_convolve_uniform_image() {
    // A uniform v comes back as v·ΣK, off by the n² weights' summing: 0.5·n²·ε.
    let width = 32;
    let height = 32;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let mut result = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.5, &mut result);

    let n2 = elliptical_gaussian_kernel_2d(2.0, 0.5, 0.5).weights.len() as f32;
    for v in &result {
        assert!((v - 0.5).abs() <= 0.5 * n2 * f32::EPSILON, "{v}");
    }
}

#[test]
fn elliptical_convolve_preserves_flux() {
    // A centred delta spreads into the kernel itself, summing to 1 within 2·n²·ε.
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(32, 32)] = 1.0;

    let mut result = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.3, &mut result);

    let n2 = elliptical_gaussian_kernel_2d(2.0, 0.5, 0.3).weights.len() as f32;
    let output_sum: f32 = result.iter().sum();
    assert!(
        (output_sum - 1.0).abs() <= 2.0 * n2 * f32::EPSILON,
        "output={output_sum}"
    );
}

#[test]
fn elliptical_convolve_spreads_point_source() {
    // A unit delta: one nonzero tap per output, so the output is the kernel itself — mirrored,
    // which at angle 0 changes nothing.
    let width = 32;
    let height = 32;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(16, 16)] = 1.0;

    let mut result = Buffer2::new_default(width, height);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.0, &mut result);

    let kernel = elliptical_gaussian_kernel_2d(2.0, 0.5, 0.0);
    let r = kernel.size / 2;
    for dy in 0..=r {
        for dx in 0..=r {
            assert_eq!(
                result[(16 + dx, 16 + dy)],
                kernel.weights[(r + dy) * kernel.size + r + dx]
            );
        }
    }
}

#[test]
fn elliptical_convolve_axis_ratio_1_matches_circular() {
    let width = 32;
    let height = 32;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    for (i, p) in pixels.iter_mut().enumerate() {
        *p = ((i * 7 + 3) % 100) as f32 / 100.0;
    }

    let sigma = 2.0;

    let mut result_circular = Buffer2::new_default(width, height);
    let mut result_elliptical = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);

    {
        result_circular
            .pixels_mut()
            .copy_from_slice(pixels.pixels());
        gaussian_convolve(&mut result_circular, sigma, &mut temp)
    };
    elliptical_gaussian_convolve(&pixels, sigma, 1.0, 0.0, &mut result_elliptical);

    // The same kernel, once as a product of normalized rows and once normalized whole: equal
    // algebraically, apart by the n² weights' rounding, n²·ε on values ≤ 1.
    let n2 = elliptical_gaussian_kernel_2d(sigma, 1.0, 0.0).weights.len() as f32;
    for (i, (&a, &b)) in result_circular
        .iter()
        .zip(result_elliptical.iter())
        .enumerate()
    {
        assert!(
            (a - b).abs() <= n2 * f32::EPSILON,
            "axis_ratio=1.0 should match circular convolution at {i}: {a} vs {b}"
        );
    }
}

#[test]
fn elliptical_convolve_rotation_invariance() {
    // A point source convolved with elliptical kernel at different angles
    // should produce different orientations but same total flux
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(32, 32)] = 1.0;

    let mut result_0 = Buffer2::new_default(width, height);
    let mut result_90 = Buffer2::new_default(width, height);

    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, 0.0, &mut result_0);
    elliptical_gaussian_convolve(&pixels, 2.0, 0.5, FRAC_PI_2, &mut result_90);

    let sum_0: f32 = result_0.iter().sum();
    let sum_90: f32 = result_90.iter().sum();

    // Both sum their kernels, 1 within 2·n²·ε each.
    let n2 = elliptical_gaussian_kernel_2d(2.0, 0.5, 0.0).weights.len() as f32;
    assert!(
        (sum_0 - sum_90).abs() <= 4.0 * n2 * f32::EPSILON,
        "Total flux should be same at different angles: {sum_0} vs {sum_90}"
    );

    // The patterns should be rotated 90 degrees
    // At angle=0, horizontal spread > vertical
    // At angle=90, vertical spread > horizontal
    let h_spread_0 = result_0.row(32)[34]; // +2 in x
    let v_spread_0 = result_0.row(34)[32]; // +2 in y

    let h_spread_90 = result_90.row(32)[34];
    let v_spread_90 = result_90.row(34)[32];

    assert!(
        h_spread_0 > v_spread_0,
        "At angle=0, horizontal spread should be larger"
    );
    assert!(
        v_spread_90 > h_spread_90,
        "At angle=90, vertical spread should be larger"
    );
}

#[test]
fn elliptical_convolve_various_axis_ratios() {
    // For a point source at (16,16), elliptical convolution should preserve flux
    let width = 32;
    let height = 32;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(16, 16)] = 1.0;

    let mut peaks = Vec::new();
    for axis_ratio in [1.0, 0.8, 0.6, 0.4, 0.2] {
        let mut result = Buffer2::new_default(width, height);
        elliptical_gaussian_convolve(&pixels, 2.0, axis_ratio, 0.0, &mut result);

        let n2 = elliptical_gaussian_kernel_2d(2.0, axis_ratio, 0.0)
            .weights
            .len() as f32;
        let sum: f32 = result.iter().sum();
        assert!(
            (sum - 1.0).abs() <= 2.0 * n2 * f32::EPSILON,
            "Flux should be 1.0 for axis_ratio={axis_ratio}: got {sum}"
        );

        let peak = result.row(16)[16];
        assert!(
            peak > 0.0,
            "Peak should be positive for axis_ratio={axis_ratio}"
        );
        peaks.push((axis_ratio, peak));
    }

    // A narrower minor axis concentrates the kernel: the peak rises as the ratio falls.
    for pair in peaks.windows(2) {
        assert!(
            pair[1].1 > pair[0].1,
            "axis_ratio {} peak {} should exceed axis_ratio {} peak {}",
            pair[1].0,
            pair[1].1,
            pair[0].0,
            pair[0].1
        );
    }
}

/// Each weight is the Gaussian's mean over its pixel. At σ 1 the centre pixel holds
/// `erf(1/(2√2))` of the light, 0.382925, and the next one `(erf(3/(2√2)) − erf(1/(2√2)))/2`,
/// 0.241730: a ratio of 0.631273. Both weights carry one f32 rounding of the division by the shared
/// sum and the ratio one more: 2ε.
#[test]
fn gaussian_kernel_known_values() {
    let kernel = gaussian_kernel_1d(1.0);
    let center = kernel.len() / 2;
    let ratio = kernel[center + 1] / kernel[center];
    let expected_ratio = 0.631_273_4f32;
    assert!(
        (ratio - expected_ratio).abs() <= 2.0 * f32::EPSILON,
        "ratio of the pixels at 1 and 0: {ratio} vs {expected_ratio}"
    );
}

#[test]
fn convolution_linearity() {
    // Convolution should be linear: conv(a + b) = conv(a) + conv(b)
    let width = 32;
    let height = 32;
    let sigma = 2.0;

    let mut pixels_a = Buffer2::new_filled(width, height, 0.0f32);
    let mut pixels_b = Buffer2::new_filled(width, height, 0.0f32);
    pixels_a[(12, 16)] = 1.0;
    pixels_b[(20, 16)] = 1.0;

    let mut pixels_sum = Buffer2::new_filled(width, height, 0.0f32);
    for i in 0..pixels_sum.len() {
        pixels_sum[i] = pixels_a[i] + pixels_b[i];
    }

    let mut result_a = Buffer2::new_default(width, height);
    let mut result_b = Buffer2::new_default(width, height);
    let mut result_sum = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);

    {
        result_a.pixels_mut().copy_from_slice(pixels_a.pixels());
        gaussian_convolve(&mut result_a, sigma, &mut temp)
    };
    {
        result_b.pixels_mut().copy_from_slice(pixels_b.pixels());
        gaussian_convolve(&mut result_b, sigma, &mut temp)
    };
    {
        result_sum.pixels_mut().copy_from_slice(pixels_sum.pixels());
        gaussian_convolve(&mut result_sum, sigma, &mut temp)
    };

    // Deltas 8 px apart, kernel radius 6: an output between them reads both, and sums the two
    // terms it has alone before scaling rather than after — two products and a sum, 4ε relative.
    for i in 0..width * height {
        let combined = result_a[i] + result_b[i];
        assert!(
            (combined - result_sum[i]).abs() <= 4.0 * f32::EPSILON * result_sum[i],
            "Convolution should be linear at {}: {} vs {}",
            i,
            combined,
            result_sum[i]
        );
    }
}

#[test]
fn convolution_scaling() {
    // conv(k * f) = k * conv(f)
    let width = 32;
    let height = 32;
    let sigma = 2.0;
    let scale = 3.5;

    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    pixels[(16, 16)] = 1.0;

    let mut pixels_scaled = Buffer2::new_filled(width, height, 0.0f32);
    for i in 0..pixels.len() {
        pixels_scaled[i] = pixels[i] * scale;
    }

    let mut result = Buffer2::new_default(width, height);
    let mut result_scaled = Buffer2::new_default(width, height);
    let mut temp = Buffer2::new_default(width, height);

    {
        result.pixels_mut().copy_from_slice(pixels.pixels());
        gaussian_convolve(&mut result, sigma, &mut temp)
    };
    {
        result_scaled
            .pixels_mut()
            .copy_from_slice(pixels_scaled.pixels());
        gaussian_convolve(&mut result_scaled, sigma, &mut temp)
    };

    // Each output is one product, K[x]·K[y] against 3.5·K[x]·K[y]: a few roundings apart, 4ε
    // relative.
    for i in 0..width * height {
        assert!(
            (result[i] * scale - result_scaled[i]).abs() <= 4.0 * f32::EPSILON * result_scaled[i],
            "Convolution should scale linearly at {}: {} vs {}",
            i,
            result[i] * scale,
            result_scaled[i]
        );
    }
}
