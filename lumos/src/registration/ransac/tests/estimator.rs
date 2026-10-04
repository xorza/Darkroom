use super::*;
use crate::math::statistics::CHI2_99_2DOF;

/// One estimation: exact points under `known`, the listed indices moved far away, at `confidences`.
#[derive(Debug)]
struct Case {
    name: &'static str,
    model: TransformType,
    known: Transform,
    points: Vec<DVec2>,
    outliers: Vec<usize>,
    /// Per-pair confidence; uniform when empty.
    confidences: Vec<f64>,
    max_sigma: f64,
    unconstrained: bool,
}

impl Case {
    fn new(name: &'static str, model: TransformType, known: Transform, points: Vec<DVec2>) -> Self {
        Self {
            name,
            model,
            known,
            points,
            outliers: Vec::new(),
            confidences: Vec::new(),
            max_sigma: 1.0,
            unconstrained: false,
        }
    }

    /// The inliers are exactly the points left in place, and the fit maps each of them onto its
    /// target to [`exact_fit_tolerance`] — tight enough that the parameters follow: a scale off by
    /// `δ` moves a point 1000 px out by `1000·δ`, so a 1e-8 px fit pins the scale to 1e-11.
    fn check(&self) {
        let mut targets = apply_all(&self.known, &self.points);
        for (k, &i) in self.outliers.iter().enumerate() {
            targets[i] = DVec2::new(5000.0 + 731.0 * k as f64, -4000.0 + 577.0 * k as f64);
        }
        let mut config = RansacConfig::default();
        if self.unconstrained {
            config.max_rotation = None;
            config.scale_range = None;
        }
        let confidences = if self.confidences.is_empty() {
            vec![1.0; self.points.len()]
        } else {
            self.confidences.clone()
        };
        let result = estimator(self.max_sigma, config)
            .estimate(
                &matches_with_confidence(&confidences),
                &self.points,
                &targets,
                self.model,
            )
            .unwrap_or_else(|failure| panic!("{}: no estimate, {failure:?}", self.name));
        let expected: Vec<usize> = (0..self.points.len())
            .filter(|i| !self.outliers.contains(i))
            .collect();
        assert_eq!(result.inliers, expected, "{}", self.name);
        let tolerance = exact_fit_tolerance(&self.points);
        for &i in &expected {
            let miss = result.transform.apply(self.points[i]).distance(targets[i]);
            assert!(
                miss <= tolerance,
                "{}: point {i} missed by {miss}",
                self.name
            );
        }
    }
}

/// Every model, with and without outliers, at the coordinate scales registration meets.
#[test]
fn ransac_recovers_every_model_and_exactly_its_inliers() {
    use TransformType::*;
    let scattered = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(7.0, 3.0),
        DVec2::new(2.0, 8.0),
        DVec2::new(9.0, 1.0),
    ];
    let mixed_scales = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1000.0, 1000.0),
        DVec2::new(1001.0, 1000.0),
        DVec2::new(1000.0, 1001.0),
        DVec2::new(5000.0, 0.0),
        DVec2::new(0.0, 5000.0),
        DVec2::new(2500.0, 2500.0),
        DVec2::new(100.0, 100.0),
    ];
    let shifted = |x, y| Transform::translation(DVec2::new(x, y));
    let far_grid: Vec<DVec2> = make_grid(5, 2, 100.0)
        .into_iter()
        .map(|p| p + DVec2::new(2000.0, 1500.0))
        .collect();
    let million_grid: Vec<DVec2> = make_grid(5, 4, 1e6);
    let cases = [
        Case::new(
            "translation",
            Translation,
            shifted(15.0, -7.0),
            scattered.clone(),
        ),
        Case::new(
            "two points",
            Translation,
            shifted(10.0, 10.0),
            scattered[..2].to_vec(),
        ),
        Case::new(
            "sub-pixel translation",
            Translation,
            shifted(0.5, -0.3),
            make_grid(5, 4, 10.0),
        ),
        Case {
            unconstrained: true,
            ..Case::new(
                "similarity at 45°",
                Similarity,
                Transform::similarity(DVec2::new(5.0, -3.0), PI / 4.0, 1.2),
                make_grid(4, 2, 10.0),
            )
        },
        Case {
            unconstrained: true,
            ..Case::new(
                "similarity on a coarse grid",
                Similarity,
                Transform::similarity(DVec2::new(10.0, -5.0), 0.2, 1.1),
                make_grid(4, 2, 50.0),
            )
        },
        Case::new(
            "similarity at 0.001 rad",
            Similarity,
            Transform::similarity(DVec2::new(5.0, 3.0), 0.001, 1.0),
            make_grid(5, 4, 100.0),
        ),
        Case::new(
            "similarity at scale 1.0001",
            Similarity,
            Transform::similarity(DVec2::new(2.0, -1.0), 0.0, 1.0001),
            make_grid(5, 4, 100.0),
        ),
        Case {
            unconstrained: true,
            ..Case::new(
                "similarity 2000 px out",
                Similarity,
                Transform::similarity(DVec2::new(50.0, -30.0), PI / 16.0, 1.05),
                far_grid,
            )
        },
        Case::new(
            "a million pixels apart",
            Translation,
            shifted(5000.0, -3000.0),
            million_grid,
        ),
        Case::new(
            "mixed scales",
            Translation,
            shifted(10.0, -5.0),
            mixed_scales,
        ),
        Case::new(
            "affine",
            Affine,
            Transform::affine([1.1, 0.2, 10.0, -0.1, 0.95, 5.0]),
            make_grid(4, 2, 25.0),
        ),
        Case::new(
            "affine shear",
            Affine,
            Transform::affine([1.0, 0.3, 10.0, 0.1, 1.0, -5.0]),
            make_grid(5, 4, 50.0),
        ),
        Case::new(
            "homography",
            Homography,
            Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 1e-4, 5e-5]),
            make_grid(4, 2, 25.0),
        ),
        Case::new(
            "homography near affine",
            Homography,
            Transform::homography([1.0, 0.1, 5.0, -0.05, 1.0, 3.0, 1e-8, 1e-8]),
            make_grid(4, 2, 25.0),
        ),
        Case {
            outliers: vec![8, 9],
            ..Case::new(
                "two outliers",
                Translation,
                shifted(5.0, 3.0),
                make_grid(5, 2, 10.0),
            )
        },
        Case {
            outliers: vec![10, 11, 12, 13],
            ..Case::new(
                "30% outliers",
                Translation,
                shifted(5.0, 3.0),
                make_grid(7, 2, 10.0),
            )
        },
        Case {
            outliers: vec![15, 16, 17, 18, 19],
            confidences: [vec![0.9; 15], vec![0.1; 5]].concat(),
            ..Case::new(
                "low-confidence outliers",
                Translation,
                shifted(10.0, 5.0),
                make_grid(5, 4, 20.0),
            )
        },
        Case {
            outliers: vec![4],
            confidences: vec![0.9, 0.9, 0.9, 0.9, 0.01],
            max_sigma: 1.67,
            ..Case::new(
                "one outlier at low confidence",
                Translation,
                shifted(50.0, -30.0),
                vec![
                    DVec2::new(100.0, 100.0),
                    DVec2::new(200.0, 100.0),
                    DVec2::new(100.0, 200.0),
                    DVec2::new(200.0, 200.0),
                    DVec2::new(150.0, 150.0),
                ],
            )
        },
    ];
    for case in &cases {
        case.check();
    }
}

/// No estimate without enough matches — none at all, or fewer than the model's minimal sample —
/// and the failure says RANSAC never ran rather than that it found no inliers.
#[test]
fn too_few_matches_give_no_estimate() {
    let estimator = estimator(1.0, RansacConfig::default());
    let never_ran = Err(RansacFailure {
        reason: RansacFailureReason::TooFewMatches,
        iterations: 0,
        best_inlier_count: 0,
    });
    assert_eq!(
        estimator
            .estimate(&[], &[], &[], TransformType::Translation)
            .map(|result| result.inliers),
        never_ran
    );
    let one = [DVec2::ZERO];
    assert_eq!(
        estimate_uniform(&estimator, &one, &one, TransformType::Similarity)
            .map(|result| result.inliers),
        never_ran
    );
}

/// A seed repeats a run: the same inliers and the same transform, bit for bit.
#[test]
fn a_seed_repeats_the_run() {
    let ref_points = make_grid(6, 5, 10.0);
    let mut target_points = apply_all(
        &Transform::similarity(DVec2::new(5.0, 3.0), 0.05, 1.01),
        &ref_points,
    );
    target_points[7] += DVec2::new(40.0, -40.0);
    let run = || {
        estimate_uniform(
            &estimator(
                1.0,
                RansacConfig {
                    seed: 12345,
                    ..Default::default()
                },
            ),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
        .unwrap()
    };
    let (first, second) = (run(), run());
    assert_eq!(first.inliers, second.inliers);
    assert_eq!(first.transform.matrix(), second.transform.matrix());
}

/// The adaptive bound stops the loop on every iteration, not only on an improvement. Half the 50
/// matches are a translation's inliers and half sit far from it, and every sample is the inlier at
/// index 0: the first hypothesis finds the 25 inliers, nothing later beats it, and
/// `adaptive_iterations(0.5, 1, 0.999)` = ⌈ln 0.001 / ln 0.5⌉ = ⌈9.97⌉ = 10 is the iteration count
/// — where a bound read only on improvement ran to the 10 000 cap.
#[test]
fn ransac_stops_at_the_adaptive_bound_without_a_later_improvement() {
    let shift = Transform::translation(DVec2::new(7.0, 3.0));
    let inliers = make_grid(5, 5, 10.0);
    let mut ref_points = inliers.clone();
    let mut target_points = apply_all(&shift, &inliers);
    for (i, &p) in make_grid(5, 5, 10.0).iter().enumerate() {
        ref_points.push(p + DVec2::new(1000.0, 0.0));
        target_points.push(p + DVec2::new(-500.0, 300.0 + i as f64));
    }

    let estimator = estimator(
        0.33,
        RansacConfig {
            max_iterations: 10_000,
            confidence: 0.999,
            ..Default::default()
        },
    );
    let mut samples = 0;
    let result = estimator
        .ransac_loop(
            &ref_points,
            &target_points,
            TransformType::Translation,
            0,
            |_, sample| {
                samples += 1;
                sample.clear();
                sample.push(0);
            },
        )
        .unwrap();
    assert_eq!(samples, 10);
    assert_eq!(result.inliers, (0..25).collect::<Vec<_>>());
    assert_eq!(
        result.transform.translation_components(),
        DVec2::new(7.0, 3.0)
    );
}

/// The final least-squares refit is kept only when it scores at least as well as the robust fit.
/// Eight points with no displacement and two displaced 3 px: all ten are inliers of the identity,
/// but their least-squares translation, the mean 0.6 px, scores worse — the two displaced points
/// lose less and the eight exact ones lose more — so the identity stands.
#[test]
fn final_refit_does_not_degrade_robust_score() {
    let ref_points: Vec<DVec2> = (0..10)
        .map(|index| DVec2::new(f64::from(index) * 10.0, 0.0))
        .collect();
    let mut target_points = ref_points.clone();
    for point in &mut target_points[8..] {
        point.x += 3.0;
    }

    let scorer = MagsacScorer::new(1.0);
    let mut inliers = Vec::new();
    let mut score = |transform| {
        score_hypothesis(
            &ref_points,
            &target_points,
            &transform,
            &scorer,
            &mut inliers,
            f64::NEG_INFINITY,
        )
    };
    let robust_score = score(Transform::identity());
    let refit_score = score(Transform::translation(DVec2::new(0.6, 0.0)));
    assert!(
        robust_score > refit_score,
        "{robust_score} against {refit_score}"
    );

    let estimator = estimator(
        1.0,
        RansacConfig {
            max_iterations: 1,
            local_optimization: false,
            ..Default::default()
        },
    );
    let result = estimator
        .ransac_loop(
            &ref_points,
            &target_points,
            TransformType::Translation,
            0,
            |_, sample| {
                sample.clear();
                sample.push(0);
            },
        )
        .unwrap();
    assert_eq!(result.inliers, (0..10).collect::<Vec<_>>());
    assert_eq!(result.transform.translation_components(), DVec2::ZERO);
}

/// Local optimization grows the consensus of a minimal sample. Ten points 100 px apart drift by
/// `0.3·k` px, and `σ_max = 1/√χ²` puts the inlier threshold at 1 px. The sample is point 0, whose
/// translation 0 reaches `0.3·k ≤ 1`: points 0–3.
///
/// Without LO the final refit takes their mean, 0.45, which reaches `|0.3·k − 0.45| ≤ 1`: points
/// 0–4. With LO each refit reaches one point further — the mean of 0–4 is 0.6, reaching 0–5; the
/// mean of 0–5 is 0.75, reaching 0–5 again (`0.3·6 − 0.75` = 1.05) — and the final refit of 0–5
/// keeps 0.75. Five inliers at 0.45 without, six at 0.75 with. The mean is taken of coordinates up
/// to 900 px, whose rounding, 1e-13, is what the shift carries.
#[test]
fn local_optimization_grows_the_consensus() {
    let ref_points: Vec<DVec2> = (0..10)
        .map(|k| DVec2::new(100.0 * f64::from(k), 0.0))
        .collect();
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .enumerate()
        .map(|(k, &p)| p + DVec2::new(0.3 * k as f64, 0.0))
        .collect();
    let max_sigma = 1.0 / CHI2_99_2DOF.sqrt();
    for (local_optimization, inliers, shift) in [(false, 5, 0.45), (true, 6, 0.75)] {
        let estimator = estimator(
            max_sigma,
            RansacConfig {
                max_iterations: 1,
                local_optimization,
                ..Default::default()
            },
        );
        let result = estimator
            .ransac_loop(
                &ref_points,
                &target_points,
                TransformType::Translation,
                0,
                |_, sample| {
                    sample.clear();
                    sample.push(0);
                },
            )
            .unwrap();
        assert_eq!(
            result.inliers,
            (0..inliers).collect::<Vec<_>>(),
            "LO {local_optimization}"
        );
        let found = result.transform.translation_components();
        assert!(
            (found.x - shift).abs() <= 4.0 * f64::EPSILON * 900.0 && found.y == 0.0,
            "LO {local_optimization}: {found:?}"
        );
    }
}

/// Confidence-weighted sampling finds the trusted pairs first. Of 100 pairs, the last 20 agree on a
/// similarity and the rest are scattered. The first guided phase lasts
/// `⌈ln 0.005 / ln 0.75⌉` = 19 iterations for a two-point sample, so a run of five samples only the
/// top quarter by confidence, weighted by `(c + 0.1)²`. With the 20 trusted at confidence 1 and the
/// rest at 0.01, that quarter holds all 20 at a weight a hundred times the rest's, and a two-point
/// sample is all trusted with probability 0.99: the 20 come back exactly. With every confidence
/// equal, the sort keeps index order and the quarter holds only scattered pairs.
#[test]
fn confidence_weighted_sampling_finds_the_trusted_pairs_first() {
    let mut rng = TestRng::new(7);
    let truth = Transform::similarity(DVec2::new(12.0, -8.0), 0.03, 1.002);
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for _ in 0..80 {
        ref_points.push(DVec2::new(rng.next_f64() * 2000.0, rng.next_f64() * 2000.0));
        target_points.push(DVec2::new(rng.next_f64() * 2000.0, rng.next_f64() * 2000.0));
    }
    for _ in 0..20 {
        let p = DVec2::new(rng.next_f64() * 2000.0, rng.next_f64() * 2000.0);
        ref_points.push(p);
        target_points.push(truth.apply(p));
    }
    let config = RansacConfig {
        max_iterations: 5,
        ..Default::default()
    };
    let trusted: Vec<usize> = (80..100).collect();

    let weighted = [vec![0.01; 80], vec![1.0; 20]].concat();
    let result = estimator(1.0, config.clone())
        .estimate(
            &matches_with_confidence(&weighted),
            &ref_points,
            &target_points,
            TransformType::Similarity,
        )
        .unwrap();
    assert_eq!(result.inliers, trusted);

    let uniform = estimator(1.0, config).estimate(
        &matches_with_confidence(&[1.0; 100]),
        &ref_points,
        &target_points,
        TransformType::Similarity,
    );
    assert_ne!(uniform.map(|result| result.inliers), Ok(trusted));
}
