use super::*;

/// Every model recovers its own transform from exact points, to rounding: from its minimal set,
/// from more, and from points spread over 5000 px where an unnormalized solve would lose digits.
#[test]
fn every_model_recovers_its_transform_exactly() {
    let square = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(2.0, 7.0),
    ];
    let wide = [
        DVec2::new(0.01, 0.02),
        DVec2::new(5000.0, 0.01),
        DVec2::new(5000.0, 4000.0),
        DVec2::new(0.01, 4000.0),
        DVec2::new(2500.0, 2000.0),
        DVec2::new(1000.0, 3000.0),
        DVec2::new(4000.0, 1000.0),
        DVec2::new(100.0, 100.0),
    ];
    let cases = [
        (
            TransformType::Translation,
            Transform::translation(DVec2::new(5.0, -3.0)),
        ),
        (
            TransformType::Euclidean,
            Transform::euclidean(DVec2::new(5.0, -3.0), PI / 12.0),
        ),
        (
            TransformType::Similarity,
            Transform::similarity(DVec2::new(20.0, -10.0), PI / 6.0, 1.5),
        ),
        (
            TransformType::Affine,
            Transform::affine([1.05, 0.02, 10.0, -0.01, 0.98, 5.0]),
        ),
        (
            TransformType::Homography,
            Transform::homography([1.05, 0.02, 10.0, -0.01, 0.98, 5.0, 1e-5, -2e-5]),
        ),
    ];
    for (model, known) in cases {
        let minimal = &square[..model.min_points()];
        for points in [minimal, &square[..], &wide[..]] {
            let targets = apply_all(&known, points);
            let estimated = estimate_transform(points, &targets, model)
                .unwrap_or_else(|| panic!("{model:?} on {} points", points.len()));
            assert_eq!(estimated.transform_type(), model);
            let tolerance = exact_fit_tolerance(points);
            for p in square.iter().chain(&wide) {
                let miss = estimated.apply(*p).distance(known.apply(*p));
                assert!(
                    miss <= tolerance,
                    "{model:?} from {} points misses {p:?} by {miss}",
                    points.len()
                );
            }
        }
    }
}

/// One point short of a model's minimal set gives no transform, for every model.
#[test]
fn too_few_points_give_no_transform() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    for model in [
        TransformType::Translation,
        TransformType::Euclidean,
        TransformType::Similarity,
        TransformType::Affine,
        TransformType::Homography,
    ] {
        let short = &points[..model.min_points() - 1];
        assert!(
            estimate_transform(short, short, model).is_none(),
            "{model:?} from {} points",
            short.len()
        );
    }
}

/// A translation fit is the mean displacement: (0, 0) → (5, −3), (10, 0) → (16, −3),
/// (0, 10) → (5, 9) and (10, 10) → (14, 5) move by (5, −3), (6, −3), (5, −1) and (4, −5), whose
/// mean (5, −3) is exact.
#[test]
fn a_translation_fit_is_the_mean_displacement() {
    let reference = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    let target = [
        DVec2::new(5.0, -3.0),
        DVec2::new(16.0, -3.0),
        DVec2::new(5.0, 9.0),
        DVec2::new(14.0, 5.0),
    ];
    let fit = estimate_transform(&reference, &target, TransformType::Translation).unwrap();
    assert_eq!(fit.translation_components(), DVec2::new(5.0, -3.0));
}

/// A Euclidean fit of scaled data keeps the rotation and drops the scale: the constrained
/// least-squares rotation of centred points `q = s·R(θ)·p` is the angle of `Σ(p × q, p · q)`, which
/// is θ whatever `s` is. Scale 1 and the angle hold to rounding.
#[test]
fn a_euclidean_fit_of_scaled_data_keeps_the_rotation() {
    let reference = make_grid(3, 3, 10.0);
    let angle = PI / 6.0;
    let target = apply_all(
        &Transform::similarity(DVec2::new(3.0, -2.0), angle, 1.1),
        &reference,
    );
    let fit = estimate_transform(&reference, &target, TransformType::Euclidean).unwrap();
    assert!(
        (fit.scale_factor() - 1.0).abs() <= 4.0 * f64::EPSILON,
        "{}",
        fit.scale_factor()
    );
    assert!(
        (fit.rotation_angle() - angle).abs() <= 4.0 * f64::EPSILON,
        "{}",
        fit.rotation_angle()
    );
}

/// A minimal sample a hair off collinear passes the normal equations' determinant floor yet fits
/// coefficients of order 1e7 once denormalized: three reference points a pixel apart, the third
/// 3e-5 px off their line, against targets thousands of pixels apart. Such a fit has no
/// normalizable product, and the estimate reports none rather than panicking in `compose`.
#[test]
fn a_near_collinear_affine_sample_gives_no_transform() {
    let reference = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.00003),
    ];
    let target = [
        DVec2::new(0.0, 0.0),
        DVec2::new(3000.0, 1000.0),
        DVec2::new(1000.0, 3000.0),
    ];
    assert!(estimate_transform(&reference, &target, TransformType::Affine).is_none());
}
