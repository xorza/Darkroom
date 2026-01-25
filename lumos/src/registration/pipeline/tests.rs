use super::*;
use std::f64::consts::PI;

fn generate_star_grid(
    rows: usize,
    cols: usize,
    spacing: f64,
    offset: (f64, f64),
) -> Vec<(f64, f64)> {
    let mut stars = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let x = offset.0 + c as f64 * spacing;
            let y = offset.1 + r as f64 * spacing;
            stars.push((x, y));
        }
    }
    stars
}

fn transform_stars(stars: &[(f64, f64)], transform: &TransformMatrix) -> Vec<(f64, f64)> {
    stars.iter().map(|&(x, y)| transform.apply(x, y)).collect()
}

#[test]
fn test_registration_identity() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let target_stars = ref_stars.clone();

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Should find near-identity transform
    let (tx, ty) = result.transform.translation_components();
    assert!(tx.abs() < 1.0, "Expected near-zero translation, got {}", tx);
    assert!(ty.abs() < 1.0, "Expected near-zero translation, got {}", ty);
    assert!(result.rms_error < 0.5, "Expected low RMS error");
}

#[test]
fn test_registration_translation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let translation = TransformMatrix::from_translation(50.0, -30.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    assert!((tx - 50.0).abs() < 1.0, "Expected tx=50, got {}", tx);
    assert!((ty - (-30.0)).abs() < 1.0, "Expected ty=-30, got {}", ty);
}

#[test]
fn test_registration_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
    let rotation = TransformMatrix::euclidean(10.0, -5.0, 0.1); // ~5.7 degrees
    let target_stars = transform_stars(&ref_stars, &rotation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

    let angle = result.transform.rotation_angle();
    assert!(
        (angle - 0.1).abs() < 0.01,
        "Expected angle=0.1, got {}",
        angle
    );
}

#[test]
fn test_registration_similarity() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
    let similarity = TransformMatrix::similarity(20.0, 15.0, 0.05, 1.02);
    let target_stars = transform_stars(&ref_stars, &similarity);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

    let scale = result.transform.scale_factor();
    assert!(
        (scale - 1.02).abs() < 0.01,
        "Expected scale=1.02, got {}",
        scale
    );
}

#[test]
fn test_registration_with_outliers() {
    let ref_stars = generate_star_grid(6, 6, 80.0, (100.0, 100.0));
    let translation = TransformMatrix::from_translation(25.0, 40.0);
    let mut target_stars = transform_stars(&ref_stars, &translation);

    // Add outliers (wrong matches)
    target_stars[0] = (500.0, 500.0);
    target_stars[5] = (50.0, 800.0);
    target_stars[10] = (900.0, 100.0);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    // RANSAC should still find correct translation despite outliers
    assert!((tx - 25.0).abs() < 2.0, "Expected tx=25, got {}", tx);
    assert!((ty - 40.0).abs() < 2.0, "Expected ty=40, got {}", ty);
}

#[test]
fn test_registration_insufficient_stars() {
    let ref_stars = vec![(100.0, 100.0), (200.0, 200.0)];
    let target_stars = ref_stars.clone();

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation);
    assert!(matches!(
        result,
        Err(RegistrationError::InsufficientStars { .. })
    ));
}

#[test]
fn test_registrator_config() {
    let config = RegistrationConfig::builder()
        .with_rotation()
        .ransac_iterations(2000)
        .ransac_threshold(1.5)
        .build();

    let registrator = Registrator::new(config);
    assert_eq!(registrator.config().ransac_iterations, 2000);
    assert!((registrator.config().ransac_threshold - 1.5).abs() < 1e-10);
}

#[test]
fn test_warp_to_reference() {
    // Create a simple test image
    let width = 64;
    let height = 64;
    let mut image = vec![0.0f32; width * height];
    image[32 * width + 32] = 1.0; // Bright pixel at center

    let transform = TransformMatrix::from_translation(5.0, 3.0);
    let warped = warp_to_reference(
        &image,
        width,
        height,
        &transform,
        InterpolationMethod::Bilinear,
    );

    assert_eq!(warped.len(), width * height);
    // The bright pixel should have moved
    assert!(warped[32 * width + 32] < 0.5);
}

#[test]
fn test_quick_register() {
    let ref_stars = generate_star_grid(4, 4, 150.0, (100.0, 100.0));
    let translation = TransformMatrix::from_translation(10.0, -15.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let transform = quick_register(&ref_stars, &target_stars).unwrap();
    let (tx, ty) = transform.translation_components();

    assert!((tx - 10.0).abs() < 1.0);
    assert!((ty - (-15.0)).abs() < 1.0);
}

#[test]
fn test_registration_result_quality() {
    let ref_stars = generate_star_grid(6, 6, 100.0, (50.0, 50.0));
    let target_stars = ref_stars.clone();

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Perfect match should have very low error and high quality
    assert!(result.rms_error < 0.1);
    assert!(result.num_inliers >= 20);
}

#[test]
fn test_registration_large_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (250.0, 250.0));
    // 30 degree rotation around image center
    let rotation = TransformMatrix::from_rotation_around(PI / 6.0, 300.0, 300.0);
    let target_stars = transform_stars(&ref_stars, &rotation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

    let angle = result.transform.rotation_angle();
    assert!(
        (angle - PI / 6.0).abs() < 0.05,
        "Expected 30deg rotation, got {} rad",
        angle
    );
}
