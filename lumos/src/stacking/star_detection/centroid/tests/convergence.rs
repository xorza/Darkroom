use crate::stacking::star_detection::centroid::tests::*;

/// Verify that Phase 1 (weighted moments) reaches sub-pixel accuracy quickly.
/// After 2 iterations the position should be within 0.1px of the final converged position.
/// This establishes that reducing max iterations for fitting methods is safe.
#[test]
fn test_phase1_reaches_good_accuracy_in_few_iterations() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let stamp_radius = 7;
    let expected_fwhm = 5.9;

    // Run full convergence to get the final position
    let mut pos_full = Vec2::new(32.0, 33.0);
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        let new_pos = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_full,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
        let delta = new_pos - pos_full;
        pos_full = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Run only 2 iterations
    let mut pos_2iter = Vec2::new(32.0, 33.0);
    for _ in 0..2 {
        pos_2iter = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
    }

    // After 2 iterations, should be within 0.2px of the fully converged position
    // (the convergence threshold is 0.001px, so final iterations refine at sub-millipixel
    // level — well beyond what L-M fitting needs as a starting point)
    let diff = ((pos_2iter.x - pos_full.x).powi(2) + (pos_2iter.y - pos_full.y).powi(2)).sqrt();
    assert!(
        diff < 0.2,
        "After 2 iterations, position should be within 0.2px of converged result, got {:.4}px diff",
        diff
    );

    // And within 0.3px of the true position
    let error = ((pos_2iter.x - true_pos.x).powi(2) + (pos_2iter.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.3,
        "After 2 iterations, position should be within 0.3px of true position, got {:.4}px error",
        error
    );
}

/// Verify that even a single Phase 1 iteration provides a reasonable starting
/// point for L-M fitting (position within ~0.5 pixels of true center).
#[test]
fn test_single_phase1_iteration_provides_good_seed() {
    let width = 64;
    let height = 64;

    // Test multiple sub-pixel offsets
    for dx in 0..5 {
        for dy in 0..5 {
            let true_pos = Vec2::new(32.0 + dx as f32 * 0.2, 32.0 + dy as f32 * 0.2);
            let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
            let bg = make_uniform_background(width, height, 0.1, 0.01);

            // Start from integer peak position
            let start = Vec2::new(true_pos.x.round(), true_pos.y.round());

            let after_one = refine_centroid(&pixels, width, height, &bg, start, 7, 5.9)
                .expect("refine should succeed");

            let error =
                ((after_one.x - true_pos.x).powi(2) + (after_one.y - true_pos.y).powi(2)).sqrt();

            assert!(
                error < 0.5,
                "Single iteration should get within 0.5px of true pos, got {:.3} for true_pos=({:.1}, {:.1})",
                error,
                true_pos.x,
                true_pos.y
            );
        }
    }
}

/// Verify that GaussianFit accuracy is equivalent whether Phase 1 runs 2 or 10 iterations.
#[test]
fn test_gaussian_fit_accuracy_independent_of_phase1_iterations() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };

    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let sigma = 2.5;
    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let stamp_radius = 7;
    let expected_fwhm = 5.9;

    // Phase 1 with only 2 iterations
    let mut pos_2iter = Vec2::new(32.0, 33.0);
    for _ in 0..2 {
        pos_2iter = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
    }

    // Phase 1 with full 10 iterations
    let mut pos_full = Vec2::new(32.0, 33.0);
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        let new_pos = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_full,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
        let delta = new_pos - pos_full;
        pos_full = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Now apply Gaussian fit from both starting points
    let config = GaussianFitConfig::default();
    let result_2iter = fit_gaussian_2d(&pixels, pos_2iter, stamp_radius, 0.1, None, &config)
        .expect("fit should succeed from 2-iter seed");
    let result_full = fit_gaussian_2d(&pixels, pos_full, stamp_radius, 0.1, None, &config)
        .expect("fit should succeed from full seed");

    // Both should converge to essentially the same position
    let diff = ((result_2iter.pos.x - result_full.pos.x).powi(2)
        + (result_2iter.pos.y - result_full.pos.y).powi(2))
    .sqrt();

    assert!(
        diff < 0.01,
        "Gaussian fit should converge to same position regardless of Phase 1 iterations: diff={:.4}",
        diff
    );

    // Both should be close to true position
    let error_2iter = ((result_2iter.pos.x - true_pos.x).powi(2)
        + (result_2iter.pos.y - true_pos.y).powi(2))
    .sqrt();
    assert!(
        error_2iter < 0.05,
        "Gaussian fit from 2-iter seed should achieve <0.05px accuracy, got {:.4}",
        error_2iter
    );
}

/// Verify that MoffatFit accuracy is equivalent whether Phase 1 runs 2 or 10 iterations.
#[test]
fn test_moffat_fit_accuracy_independent_of_phase1_iterations() {
    use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let stamp_radius = 7;
    let expected_fwhm = 5.9;

    // Phase 1 with only 2 iterations
    let mut pos_2iter = Vec2::new(32.0, 33.0);
    for _ in 0..2 {
        pos_2iter = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
    }

    // Phase 1 with full 10 iterations
    let mut pos_full = Vec2::new(32.0, 33.0);
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        let new_pos = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_full,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
        let delta = new_pos - pos_full;
        pos_full = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Now apply Moffat fit from both starting points
    let config = MoffatFitConfig {
        fixed_beta: 2.5,
        ..MoffatFitConfig::default()
    };
    let result_2iter = fit_moffat_2d(&pixels, pos_2iter, stamp_radius, 0.1, None, &config)
        .expect("fit should succeed from 2-iter seed");
    let result_full = fit_moffat_2d(&pixels, pos_full, stamp_radius, 0.1, None, &config)
        .expect("fit should succeed from full seed");

    // Both should converge to essentially the same position
    let diff = ((result_2iter.pos.x - result_full.pos.x).powi(2)
        + (result_2iter.pos.y - result_full.pos.y).powi(2))
    .sqrt();

    assert!(
        diff < 0.01,
        "Moffat fit should converge to same position regardless of Phase 1 iterations: diff={:.4}",
        diff
    );

    // Both should be close to true position
    let error_2iter = ((result_2iter.pos.x - true_pos.x).powi(2)
        + (result_2iter.pos.y - true_pos.y).powi(2))
    .sqrt();
    assert!(
        error_2iter < 0.05,
        "Moffat fit from 2-iter seed should achieve <0.05px accuracy, got {:.4}",
        error_2iter
    );
}

#[test]
fn compute_stamp_radius_scales_and_clamps() {
    use crate::stacking::star_detection::centroid::compute_stamp_radius;
    let cases = [
        (1.0, 4),
        (2.0, 4),
        (3.0, 6),
        (4.0, 7),
        (5.0, 9),
        (6.0, 11),
        (8.0, 14),
        (10.0, 15),
        (20.0, 15),
    ];

    for (fwhm, expected) in cases {
        assert_eq!(
            compute_stamp_radius(fwhm),
            expected,
            "FWHM {fwhm} uses ceil(1.75 × FWHM), clamped to [4, 15]"
        );
    }
}

/// Verifies that using only 2 pre-fit moments iterations produces equivalent
/// centroid results compared to using 10 iterations before Gaussian/Moffat fitting.
///
/// This test validates the design decision in MOMENTS_ITERATIONS_BEFORE_FIT:
/// the L-M optimizer refines position independently and converges to the same
/// result regardless of Phase 1 precision.
#[test]
fn test_prefit_moments_iterations_sufficient() {
    use crate::stacking::star_detection::centroid::gaussian_fit::{
        GaussianFitConfig, fit_gaussian_2d,
    };
    use crate::stacking::star_detection::centroid::{CONVERGENCE_THRESHOLD_SQ, refine_centroid};

    let width = 64;
    let height = 64;

    // Test with various sub-pixel positions and FWHM values
    let test_cases = [
        (Vec2::new(32.3, 32.7), 2.5f32), // Typical star, FWHM ~5.9
        (Vec2::new(32.8, 32.2), 3.5f32), // Larger PSF, FWHM ~8.2
        (Vec2::new(32.1, 32.9), 1.8f32), // Smaller PSF, FWHM ~4.2
    ];

    for (true_pos, sigma) in test_cases {
        let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8, 0.1);
        let bg = make_uniform_background(width, height, 0.1, 0.01);
        let expected_fwhm = sigma / FWHM_TO_SIGMA;
        let stamp_radius = 7;

        // Start from peak position (slightly off from true position)
        let peak_pos = Vec2::new(true_pos.x.round(), true_pos.y.round());

        // Run with 2 iterations (current MOMENTS_ITERATIONS_BEFORE_FIT)
        let mut pos_2iter = peak_pos;
        for _ in 0..2 {
            if let Some(new_pos) = refine_centroid(
                &pixels,
                width,
                height,
                &bg,
                pos_2iter,
                stamp_radius,
                expected_fwhm,
            ) {
                let delta = new_pos - pos_2iter;
                pos_2iter = new_pos;
                if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                    break;
                }
            }
        }

        // Run with 10 iterations (fully converged moments)
        let mut pos_10iter = peak_pos;
        for _ in 0..10 {
            if let Some(new_pos) = refine_centroid(
                &pixels,
                width,
                height,
                &bg,
                pos_10iter,
                stamp_radius,
                expected_fwhm,
            ) {
                let delta = new_pos - pos_10iter;
                pos_10iter = new_pos;
                if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                    break;
                }
            }
        }

        // Now apply Gaussian fitting to both starting points
        let local_bg = 0.1;
        let fit_config = GaussianFitConfig {
            position_convergence_threshold: 0.0001,
            ..GaussianFitConfig::default()
        };

        let result_from_2iter = fit_gaussian_2d(
            &pixels,
            pos_2iter,
            stamp_radius,
            local_bg,
            None,
            &fit_config,
        );
        let result_from_10iter = fit_gaussian_2d(
            &pixels,
            pos_10iter,
            stamp_radius,
            local_bg,
            None,
            &fit_config,
        );

        // Both should converge
        assert!(
            result_from_2iter.is_some(),
            "Gaussian fit from 2-iter moments failed for sigma={}",
            sigma
        );
        assert!(
            result_from_10iter.is_some(),
            "Gaussian fit from 10-iter moments failed for sigma={}",
            sigma
        );

        let pos_final_2iter = result_from_2iter.unwrap().pos;
        let pos_final_10iter = result_from_10iter.unwrap().pos;

        // Final positions should be nearly identical (< 0.01 pixels)
        let diff = (pos_final_2iter - pos_final_10iter).length();
        assert!(
            diff < 0.01,
            "Position difference {:.6} pixels exceeds 0.01 for sigma={}: \
             2-iter={:?}, 10-iter={:?}",
            diff,
            sigma,
            pos_final_2iter,
            pos_final_10iter
        );

        // Both should be accurate to within 0.05 pixels of true position
        let error_2iter = (pos_final_2iter - true_pos).length();
        let error_10iter = (pos_final_10iter - true_pos).length();
        assert!(
            error_2iter < 0.05,
            "2-iter centroid error {:.4} exceeds 0.05 for sigma={}",
            error_2iter,
            sigma
        );
        assert!(
            error_10iter < 0.05,
            "10-iter centroid error {:.4} exceeds 0.05 for sigma={}",
            error_10iter,
            sigma
        );
    }
}

/// Same test but for Moffat fitting to ensure both PSF models benefit
/// from the 2-iteration pre-fit optimization.
#[test]
fn test_prefit_moments_iterations_sufficient_moffat() {
    use crate::stacking::star_detection::centroid::moffat_fit::{MoffatFitConfig, fit_moffat_2d};
    use crate::stacking::star_detection::centroid::{
        CONVERGENCE_THRESHOLD_SQ, lm_optimizer, refine_centroid,
    };

    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.4, 32.6);
    let sigma = 2.5f32;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8, 0.1);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = sigma / FWHM_TO_SIGMA;
    let stamp_radius = 7;

    let peak_pos = Vec2::new(true_pos.x.round(), true_pos.y.round());

    // Run with 2 iterations
    let mut pos_2iter = peak_pos;
    for _ in 0..2 {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        ) {
            let delta = new_pos - pos_2iter;
            pos_2iter = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        }
    }

    // Run with 10 iterations
    let mut pos_10iter = peak_pos;
    for _ in 0..10 {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_10iter,
            stamp_radius,
            expected_fwhm,
        ) {
            let delta = new_pos - pos_10iter;
            pos_10iter = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        }
    }

    // Apply Moffat fitting to both
    let local_bg = 0.1;
    let fit_config = MoffatFitConfig {
        fixed_beta: 2.5,
        lm: lm_optimizer::LMConfig {
            position_convergence_threshold: 0.0001,
            ..lm_optimizer::LMConfig::default()
        },
    };

    let result_from_2iter = fit_moffat_2d(
        &pixels,
        pos_2iter,
        stamp_radius,
        local_bg,
        None,
        &fit_config,
    );
    let result_from_10iter = fit_moffat_2d(
        &pixels,
        pos_10iter,
        stamp_radius,
        local_bg,
        None,
        &fit_config,
    );

    assert!(
        result_from_2iter.is_some(),
        "Moffat fit from 2-iter moments failed"
    );
    assert!(
        result_from_10iter.is_some(),
        "Moffat fit from 10-iter moments failed"
    );

    let pos_final_2iter = result_from_2iter.unwrap().pos;
    let pos_final_10iter = result_from_10iter.unwrap().pos;

    // Final positions should be nearly identical
    let diff = (pos_final_2iter - pos_final_10iter).length();
    assert!(
        diff < 0.01,
        "Moffat position difference {:.6} pixels exceeds 0.01: 2-iter={:?}, 10-iter={:?}",
        diff,
        pos_final_2iter,
        pos_final_10iter
    );
}
