use super::*;

/// The score is the negated MAGSAC loss, and the inliers are the points within the threshold: two
/// exact points lose nothing, and a third 475 px off loses the outlier loss `σ²/2` = 0.5.
#[test]
fn score_hypothesis_hand_values() {
    let ref_pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
    ];
    let target_pts = [
        DVec2::new(5.0, 0.0),
        DVec2::new(15.0, 0.0),
        DVec2::new(500.0, 0.0),
    ];
    let transform = Transform::translation(DVec2::new(5.0, 0.0));
    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();
    for (count, score, expected_inliers) in [(2, 0.0, vec![0, 1]), (3, -0.5, vec![0, 1])] {
        let actual = score_hypothesis(
            &ref_pts[..count],
            &target_pts[..count],
            &transform,
            &scorer,
            &mut inliers,
            f64::NEG_INFINITY,
        );
        assert_eq!(actual, score);
        assert_eq!(inliers, expected_inliers);
    }
}

/// Scoring stops as soon as the loss passes what the best hypothesis lost. Three outliers lead
/// 97 exact points; each outlier loses 0.5, and the exit test is strict, so against a best of −1
/// the loss reaches 1.0 at the second point — not past it — and 1.5 at the third, where scoring
/// stops: score −1.5, and none of the 97 inliers seen.
#[test]
fn score_hypothesis_stops_once_it_cannot_win() {
    let ref_pts: Vec<DVec2> = (0..100).map(|i| DVec2::new(f64::from(i), 0.0)).collect();
    let target_pts: Vec<DVec2> = ref_pts
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            if i < 3 {
                p + DVec2::new(1000.0, 0.0)
            } else {
                p
            }
        })
        .collect();
    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();
    let score = score_hypothesis(
        &ref_pts,
        &target_pts,
        &Transform::identity(),
        &scorer,
        &mut inliers,
        -1.0,
    );
    assert_eq!(score, -1.5);
    assert!(inliers.is_empty());

    let full = score_hypothesis(
        &ref_pts,
        &target_pts,
        &Transform::identity(),
        &scorer,
        &mut inliers,
        f64::NEG_INFINITY,
    );
    assert_eq!(full, -1.5);
    assert_eq!(inliers, (3..100).collect::<Vec<_>>());
}
