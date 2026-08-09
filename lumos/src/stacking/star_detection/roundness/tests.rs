use crate::stacking::star_detection::roundness::Roundness;

#[test]
fn roundness_zero_flux() {
    // When all marginal values are zero, roundness should be 0
    let marginal_x = vec![0.0f64; 11];
    let marginal_y = vec![0.0f64; 11];

    let roundness = Roundness::from_marginals(&marginal_x, &marginal_y);

    assert_eq!(roundness.ground, 0.0, "GROUND should be 0 for zero flux");
    assert_eq!(roundness.sround, 0.0, "SROUND should be 0 for zero flux");
}

#[test]
fn roundness_uniform_marginals() {
    // Uniform marginals should give GROUND = 0 (Hx = Hy)
    let marginal_x = vec![1.0f64; 11];
    let marginal_y = vec![1.0f64; 11];

    let roundness = Roundness::from_marginals(&marginal_x, &marginal_y);

    assert!(
        roundness.ground.abs() < 0.01,
        "GROUND should be ~0 for uniform marginals"
    );
}

#[test]
fn roundness_asymmetric_x() {
    // Create asymmetric x marginal (more flux on right)
    let mut marginal_x = vec![0.1f64; 11];
    marginal_x[8] = 1.0; // Extra flux on right side
    let marginal_y = vec![0.5f64; 11]; // Symmetric

    let roundness = Roundness::from_marginals(&marginal_x, &marginal_y);

    // Index 5 is the excluded center. x: left = 5×0.1 = 0.5, right = 4×0.1 + 1.0 = 1.4,
    // so asym_x = (1.4 - 0.5) / 1.9 = 9/19. y is symmetric, so asym_y = 0 and
    // SROUND = hypot(9/19, 0) = 9/19.
    assert!(
        (roundness.sround - 9.0 / 19.0).abs() < 1e-6,
        "SROUND should be 9/19 for this asymmetry, got {}",
        roundness.sround
    );
}

#[test]
fn roundness_x_vs_y_elongation() {
    // X-elongated: higher peak in y marginal (more compact in y)
    let mut marginal_x = vec![0.1f64; 11];
    let mut marginal_y = vec![0.1f64; 11];

    // Y marginal has higher peak (star is more compact in y, elongated in x)
    marginal_y[5] = 2.0;
    marginal_x[5] = 1.0;

    let roundness = Roundness::from_marginals(&marginal_x, &marginal_y);

    // GROUND = (Hx - Hy) / (Hx + Hy) = (1.0 - 2.0) / (1.0 + 2.0) = -1/3
    assert!(
        (roundness.ground + 1.0 / 3.0).abs() < 1e-6,
        "X-elongated star should have GROUND -1/3, got {}",
        roundness.ground
    );
}

#[test]
fn roundness_y_vs_x_elongation() {
    // Y-elongated: higher peak in x marginal (more compact in x)
    let mut marginal_x = vec![0.1f64; 11];
    let mut marginal_y = vec![0.1f64; 11];

    // X marginal has higher peak (star is more compact in x, elongated in y)
    marginal_x[5] = 2.0;
    marginal_y[5] = 1.0;

    let roundness = Roundness::from_marginals(&marginal_x, &marginal_y);

    // GROUND = (Hx - Hy) / (Hx + Hy) = (2.0 - 1.0) / (2.0 + 1.0) = 1/3
    assert!(
        (roundness.ground - 1.0 / 3.0).abs() < 1e-6,
        "Y-elongated star should have GROUND 1/3, got {}",
        roundness.ground
    );
}

#[test]
fn roundness_bounds() {
    // Test that roundness values are always within bounds
    let test_cases = [
        (vec![1.0f64; 11], vec![0.001f64; 11]), // Very different peaks
        (vec![0.001f64; 11], vec![1.0f64; 11]), // Opposite
        (vec![1.0f64; 11], vec![1.0f64; 11]),   // Equal
    ];

    for (marginal_x, marginal_y) in test_cases {
        let roundness = Roundness::from_marginals(&marginal_x, &marginal_y);
        assert!(
            (-1.0..=1.0).contains(&roundness.ground),
            "GROUND out of bounds: {}",
            roundness.ground
        );
        assert!(
            (0.0..=1.0).contains(&roundness.sround),
            "SROUND out of bounds: {}",
            roundness.sround
        );
    }
}
