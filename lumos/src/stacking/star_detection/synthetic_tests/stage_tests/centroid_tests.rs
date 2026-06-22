//! Centroid stage tests.
//!
//! Sub-pixel centroid accuracy on synthetic stars: exact known positions are recovered to a
//! stated sub-pixel tolerance, accuracy tracks SNR, and the three centroid methods
//! (weighted-moments / Gaussian-fit / Moffat-fit) agree and the profile fits beat moments.

use super::background_estimate;
use crate::math::bbox::Aabb;
use crate::math::fwhm_to_sigma;
use crate::stacking::star_detection::centroid::measure_star;
use crate::stacking::star_detection::config::{CentroidMethod, Config};
use crate::stacking::star_detection::deblend::region::Region;
use crate::testing::TestRng;
use crate::testing::synthetic::star_profiles::render_gaussian_star;
use common::Vec2us;
use imaginarium::Buffer2;

/// Render `stars` as `(x, y, brightness)` Gaussians of width `sigma` on a 0.1 sky + Gaussian
/// noise σ `noise`.
fn field(
    width: usize,
    height: usize,
    sigma: f32,
    stars: &[(f32, f32, f32)],
    noise: f32,
    seed: u64,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];
    for &(x, y, brightness) in stars {
        let amplitude = brightness / (2.0 * std::f32::consts::PI * sigma * sigma);
        render_gaussian_star(&mut pixels, width, x, y, sigma, amplitude);
    }
    let mut rng = TestRng::new(seed);
    for p in &mut pixels {
        *p += rng.next_gaussian_f32() * noise;
        *p = p.clamp(0.0, 1.0);
    }
    Buffer2::new(width, height, pixels)
}

/// Build a 11×11 candidate region centred on the pixel nearest `(x, y)`.
fn candidate_at(pixels: &Buffer2<f32>, x: f32, y: f32) -> Region {
    let (w, h) = (pixels.width(), pixels.height());
    let (px, py) = (x.round() as usize, y.round() as usize);
    Region {
        bbox: Aabb::new(
            Vec2us::new(px.saturating_sub(5), py.saturating_sub(5)),
            Vec2us::new((px + 5).min(w - 1), (py + 5).min(h - 1)),
        ),
        peak: Vec2us::new(px, py),
        peak_value: pixels[(px, py)],
        area: 50,
    }
}

#[test]
fn centroid_recovers_known_subpixel_positions() {
    let (width, height) = (256, 256);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let positions = [
        (50.0, 50.0),     // integer
        (100.3, 50.2),    // sub-pixel x
        (50.7, 100.8),    // sub-pixel y
        (150.5, 150.5),   // half-pixel both
        (100.25, 100.75), // quarter-pixel
    ];
    // Bright stars (high SNR) so centroiding is limited by sampling, not noise.
    let stars: Vec<(f32, f32, f32)> = positions.iter().map(|&(x, y)| (x, y, 5.0)).collect();
    let pixels = field(width, height, sigma, &stars, 0.01, 42);
    let background = background_estimate(&pixels);
    let config = Config {
        expected_fwhm: fwhm,
        ..Default::default()
    };

    let mut max_error = 0.0f64;
    for &(true_x, true_y) in &positions {
        let star = measure_star(
            &pixels,
            &background,
            &candidate_at(&pixels, true_x, true_y),
            &config,
        )
        .unwrap_or_else(|| panic!("no centroid at ({true_x}, {true_y})"));
        let error =
            ((star.pos.x - true_x as f64).powi(2) + (star.pos.y - true_y as f64).powi(2)).sqrt();
        println!(
            "({true_x:.2},{true_y:.2}) -> ({:.3},{:.3}) err {error:.4}",
            star.pos.x, star.pos.y
        );
        max_error = max_error.max(error);
    }
    // Bright, well-sampled Gaussians: every centroid is recovered to a small fraction of a pixel.
    assert!(
        max_error < 0.15,
        "max centroid error {max_error:.4} should be sub-0.15 px"
    );
}

#[test]
fn centroid_accuracy_improves_with_snr() {
    let (width, height) = (256, 128);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    // Descending brightness at a fixed sub-pixel position (bright → near the noise floor, but
    // all still centroidable).
    let brightnesses = [5.0f32, 2.5, 1.5, 1.0];
    let y = 64.37;
    let stars: Vec<(f32, f32, f32)> = brightnesses
        .iter()
        .enumerate()
        .map(|(i, &b)| (40.0 + i as f32 * 60.0 + 0.42, y, b))
        .collect();
    let pixels = field(width, height, sigma, &stars, 0.01, 42);
    let background = background_estimate(&pixels);
    let config = Config {
        expected_fwhm: fwhm,
        ..Default::default()
    };

    let mut measured = Vec::new();
    for &(tx, ty, _) in &stars {
        let star = measure_star(
            &pixels,
            &background,
            &candidate_at(&pixels, tx, ty),
            &config,
        )
        .expect("centroid");
        let error = ((star.pos.x - tx as f64).powi(2) + (star.pos.y - ty as f64).powi(2)).sqrt();
        measured.push((star.snr, error));
        println!("brightness target: SNR {:.1}, error {error:.4}", star.snr);
    }

    // SNR decreases with brightness, monotonically.
    for w in measured.windows(2) {
        assert!(
            w[1].0 < w[0].0,
            "SNR must fall with brightness: {:.1} then {:.1}",
            w[0].0,
            w[1].0
        );
    }
    // The brightest (highest-SNR) star is recovered sub-0.1 px.
    assert!(
        measured[0].1 < 0.1,
        "brightest centroid error {:.4} should be sub-0.1 px",
        measured[0].1
    );
    // The faintest (lowest SNR) is no better than a brighter one — error grows as SNR drops.
    assert!(
        measured[3].1 >= measured[0].1,
        "faint centroid error {:.4} should not beat the brightest {:.4}",
        measured[3].1,
        measured[0].1
    );
}

#[test]
fn centroid_methods_agree_and_fits_beat_moments() {
    let (width, height) = (128, 128);
    let fwhm = 4.0;
    let sigma = fwhm_to_sigma(fwhm);
    let (tx, ty) = (64.37f32, 64.63f32);
    // A bright, clean star so all three methods are in their accurate regime.
    let pixels = field(width, height, sigma, &[(tx, ty, 5.0)], 0.005, 7);
    let background = background_estimate(&pixels);

    let error_for = |method: CentroidMethod| -> (f64, f64) {
        let config = Config {
            expected_fwhm: fwhm,
            centroid_method: method,
            ..Default::default()
        };
        let star = measure_star(
            &pixels,
            &background,
            &candidate_at(&pixels, tx, ty),
            &config,
        )
        .expect("centroid");
        let err = ((star.pos.x - tx as f64).powi(2) + (star.pos.y - ty as f64).powi(2)).sqrt();
        (star.pos.x, err)
    };

    let (wm_x, wm_err) = error_for(CentroidMethod::WeightedMoments);
    let (gf_x, gf_err) = error_for(CentroidMethod::GaussianFit);
    let (mf_x, mf_err) = error_for(CentroidMethod::MoffatFit { beta: 2.5 });
    println!("errors — moments {wm_err:.4}, gaussian {gf_err:.4}, moffat {mf_err:.4}");

    // All three land within a small fraction of a pixel of truth and of each other.
    assert!(wm_err < 0.1, "weighted-moments error {wm_err:.4}");
    assert!(gf_err < 0.05, "gaussian-fit error {gf_err:.4}");
    assert!(mf_err < 0.05, "moffat-fit error {mf_err:.4}");
    assert!(
        (gf_x - mf_x).abs() < 0.05 && (gf_x - wm_x).abs() < 0.1,
        "methods should agree on x: wm {wm_x:.3}, gf {gf_x:.3}, mf {mf_x:.3}"
    );
    // The profile fits advertise ~0.01 px; they should be at least as accurate as moments.
    assert!(
        gf_err <= wm_err && mf_err <= wm_err,
        "profile fits should match or beat moments: wm {wm_err:.4}, gf {gf_err:.4}, mf {mf_err:.4}"
    );
}
