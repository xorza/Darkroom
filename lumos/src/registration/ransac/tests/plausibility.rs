use super::*;

/// A hypothesis is plausible when its rotation is at most `max_rotation` in magnitude and its scale
/// inside `scale_range`, either check off when its limit is `None`.
#[test]
fn plausibility_hand_cases() {
    let ten_degrees = Some(10.0f64.to_radians());
    let usual = Some((0.8, 1.2));
    let similarity = |angle_deg: f64, scale| {
        Transform::similarity(DVec2::new(5.0, -3.0), angle_deg.to_radians(), scale)
    };
    for (transform, max_rotation, scale_range, plausible, why) in [
        (
            similarity(9.9, 1.0),
            ten_degrees,
            None,
            true,
            "9.9° inside 10°",
        ),
        (
            similarity(10.5, 1.0),
            ten_degrees,
            None,
            false,
            "10.5° past 10°",
        ),
        (
            similarity(-10.5, 1.0),
            ten_degrees,
            None,
            false,
            "−10.5° past 10°",
        ),
        (
            similarity(0.0, 1.15),
            None,
            usual,
            true,
            "1.15 inside (0.8, 1.2)",
        ),
        (similarity(0.0, 1.25), None, usual, false, "1.25 above 1.2"),
        (similarity(0.0, 0.75), None, usual, false, "0.75 below 0.8"),
        (
            similarity(5.0, 1.5),
            ten_degrees,
            usual,
            false,
            "rotation fine, scale not",
        ),
        (
            similarity(15.0, 1.05),
            ten_degrees,
            usual,
            false,
            "scale fine, rotation not",
        ),
        (similarity(5.0, 1.05), ten_degrees, usual, true, "both fine"),
        (similarity(45.0, 3.0), None, None, true, "both checks off"),
        (
            Transform::translation(DVec2::new(100.0, -50.0)),
            Some(1.0f64.to_radians()),
            Some((0.99, 1.01)),
            true,
            "a translation has rotation 0 and scale 1",
        ),
    ] {
        let estimator = estimator(
            1.0,
            RansacConfig {
                max_rotation,
                scale_range,
                ..Default::default()
            },
        );
        assert_eq!(estimator.is_plausible(&transform), plausible, "{why}");
    }
}

/// The 10° rotation limit the plausibility tests set; the default sets none.
fn ten_degree_limit() -> RansacConfig {
    RansacConfig {
        max_rotation: Some(10.0f64.to_radians()),
        ..Default::default()
    }
}

/// `estimate` holds every hypothesis to the limits: a 30° similarity has no plausible fit under a
/// 10° limit, and a 5° one at scale 1.05 fits all 20 points.
#[test]
fn estimate_honours_the_plausibility_limits() {
    let ref_points = make_grid(5, 4, 50.0);
    let estimator = estimator(1.0, ten_degree_limit());
    let rotated = apply_all(
        &Transform::similarity(DVec2::new(5.0, -3.0), 30.0f64.to_radians(), 1.0),
        &ref_points,
    );
    // Every hypothesis is implausible, so none is scored: the whole budget runs and the best
    // gathered no inliers.
    assert_eq!(
        estimate_uniform(&estimator, &ref_points, &rotated, TransformType::Similarity)
            .map(|result| result.inliers),
        Err(RansacFailure {
            reason: RansacFailureReason::NoInliersFound,
            iterations: RansacConfig::default().max_iterations,
            best_inlier_count: 0,
        })
    );
    let mild = apply_all(
        &Transform::similarity(DVec2::new(5.0, -3.0), 5.0f64.to_radians(), 1.05),
        &ref_points,
    );
    let result =
        estimate_uniform(&estimator, &ref_points, &mild, TransformType::Similarity).unwrap();
    assert_eq!(result.inliers, (0..20).collect::<Vec<_>>());
}

/// The limits decide between two consistent groups. Twenty pairs agree on a 2° similarity and
/// thirty on a quarter turn: with the limits off, the larger group wins; with a 10° limit, its
/// every hypothesis is refused and the smaller, plausible group is the answer.
#[test]
fn the_limits_steer_ransac_to_the_plausible_group() {
    let plausible = Transform::similarity(DVec2::new(10.0, -5.0), 2.0f64.to_radians(), 1.01);
    let quarter_turn = Transform::similarity(DVec2::new(3000.0, 0.0), PI / 2.0, 1.0);
    let small = make_grid(5, 4, 50.0);
    let large: Vec<DVec2> = make_grid(6, 5, 50.0)
        .into_iter()
        .map(|p| p + DVec2::new(1000.0, 1000.0))
        .collect();
    let ref_points: Vec<DVec2> = small.iter().chain(&large).copied().collect();
    let target_points: Vec<DVec2> = apply_all(&plausible, &small)
        .into_iter()
        .chain(apply_all(&quarter_turn, &large))
        .collect();

    let limited = estimator(1.0, ten_degree_limit());
    let result = estimate_uniform(
        &limited,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();
    assert_eq!(result.inliers, (0..20).collect::<Vec<_>>());

    let unlimited = estimator(
        1.0,
        RansacConfig {
            max_rotation: None,
            scale_range: None,
            ..Default::default()
        },
    );
    let result = estimate_uniform(
        &unlimited,
        &ref_points,
        &target_points,
        TransformType::Similarity,
    )
    .unwrap();
    assert_eq!(result.inliers, (20..50).collect::<Vec<_>>());
}
