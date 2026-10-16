use std::f64::consts::PI;

use glam::DVec2;

use crate::registration::estimate::Pairs;
use crate::registration::transform::{Transform, TransformType};

/// A `cols × rows` grid at `spacing`, row by row from the origin.
fn make_grid(cols: usize, rows: usize, spacing: f64) -> Vec<DVec2> {
    let mut points = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            points.push(DVec2::new(c as f64 * spacing, r as f64 * spacing));
        }
    }
    points
}

fn apply_all(transform: &Transform, points: &[DVec2]) -> Vec<DVec2> {
    points.iter().map(|&p| transform.apply(p)).collect()
}

/// How far an exact fit of exact points may miss them: each solve is centred or
/// Hartley-normalized, which holds its condition number to the hundreds for the point sets here,
/// against coordinates up to 5000 px whose f64 rounding is 9e-13 px — so ~1e-9 px at worst, and
/// 1e-8 leaves margin.
fn exact_fit_tolerance(points: &[DVec2]) -> f64 {
    let largest = points
        .iter()
        .fold(1.0f64, |m, p| m.max(p.abs().max_element()));
    1e-8 * (largest / 5000.0).max(1.0)
}

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
            let estimated = Pairs::unit(points, &targets)
                .estimate(model)
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
            Pairs::unit(short, short).estimate(model).is_none(),
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
    let fit = Pairs::unit(&reference, &target)
        .estimate(TransformType::Translation)
        .unwrap();
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
    let fit = Pairs::unit(&reference, &target)
        .estimate(TransformType::Euclidean)
        .unwrap();
    let geometry = fit.local_geometry(DVec2::ZERO);
    for stretch in geometry.stretch {
        assert!((stretch - 1.0).abs() <= 4.0 * f64::EPSILON, "{geometry:?}");
    }
    assert!(
        (geometry.rotation - angle).abs() <= 4.0 * f64::EPSILON,
        "{geometry:?}"
    );
}

/// The weights decide the fit. A translation is the weighted mean displacement: (0, 0) → (1, 0)
/// at weight 3 and (0, 0) → (5, 0) at weight 1 give (3·1 + 5)/4 = 2, where at unit weights the
/// mean is 3.
#[test]
fn a_weighted_translation_is_the_weighted_mean_displacement() {
    let reference = [DVec2::ZERO, DVec2::ZERO];
    let target = [DVec2::new(1.0, 0.0), DVec2::new(5.0, 0.0)];
    let fit = |pairs: Pairs<'_>| {
        pairs
            .estimate(TransformType::Translation)
            .unwrap()
            .translation_components()
    };
    assert_eq!(
        fit(Pairs::weighted(&reference, &target, &[3.0, 1.0])),
        DVec2::new(2.0, 0.0)
    );
    assert_eq!(fit(Pairs::unit(&reference, &target)), DVec2::new(3.0, 0.0));
}

/// Weights that are all alike are no weights: every model fitted to noisy pairs at weight 2.5
/// lands where it does at unit weights, to rounding. And one pair weighted up pulls each fit
/// towards it: the residual of pair 0 shrinks once it weighs 100.
#[test]
fn equal_weights_fit_as_unit_weights_and_a_heavy_pair_pulls_the_fit() {
    let reference = make_grid(4, 3, 300.0);
    let truth = Transform::homography([1.02, 0.03, 12.0, -0.02, 0.97, -7.0, 2e-5, -1e-5]);
    let target: Vec<DVec2> = reference
        .iter()
        .enumerate()
        .map(|(i, &r)| {
            let wobble = (i as f64 * 1.7).sin();
            truth.apply(r) + DVec2::new(0.4 * wobble, -0.3 * wobble)
        })
        .collect();
    let equal = vec![2.5; reference.len()];
    let mut heavy = vec![1.0; reference.len()];
    heavy[0] = 100.0;
    for model in [
        TransformType::Translation,
        TransformType::Euclidean,
        TransformType::Similarity,
        TransformType::Affine,
        TransformType::Homography,
    ] {
        let unit = Pairs::unit(&reference, &target).estimate(model).unwrap();
        let alike = Pairs::weighted(&reference, &target, &equal)
            .estimate(model)
            .unwrap();
        for &p in &reference {
            let miss = unit.apply(p).distance(alike.apply(p));
            assert!(miss <= 1e-9, "{model:?} at {p:?} moves {miss}");
        }
        let pulled = Pairs::weighted(&reference, &target, &heavy)
            .estimate(model)
            .unwrap();
        let residual = |fit: &Transform| fit.apply(reference[0]).distance(target[0]);
        assert!(
            residual(&pulled) < residual(&unit),
            "{model:?}: {} not below {}",
            residual(&pulled),
            residual(&unit)
        );
    }
}

/// Each model's degeneracy rule, on either side of it.
///
/// Procrustes refuses a reference spread about the centroid at most `ε` times the spread about
/// the origin. Two points at (1000, 1000) and (1000 + δ, 1000) spread `δ²/2` about their centroid
/// against `2·1000²·2 = 4e6` about the origin, so the bound is `δ ≤ √(8e6·ε) ≈ 4.2e-5`: δ = 1e-5
/// is refused and δ = 1e-3 fitted, by the Euclidean and the similarity fit alike.
///
/// The affine fit refuses `det C ≤ √ε·(tr C)²`. Points (0, 0), (1, 0), (2, δ) centre to x of
/// −1, 0, 1 and y of −δ/3, −δ/3, 2δ/3: `C = [2, δ; δ, 2δ²/3]`, `det C = δ²/3` against
/// `(tr C)² ≈ 4`, so the bound is `δ ≈ √(12·√ε) ≈ 4.2e-4`: δ = 3e-5 is refused and δ = 1e-3
/// fitted.
#[test]
fn each_model_refuses_points_on_its_side_of_the_degeneracy_rule() {
    let pair = |delta: f64| {
        [
            DVec2::new(1000.0, 1000.0),
            DVec2::new(1000.0 + delta, 1000.0),
        ]
    };
    let shifted = |points: [DVec2; 2]| points.map(|p| p + DVec2::new(3.0, -2.0));
    for model in [TransformType::Euclidean, TransformType::Similarity] {
        let (near, far) = (pair(1e-5), pair(1e-3));
        assert!(
            Pairs::unit(&near, &shifted(near)).estimate(model).is_none(),
            "{model:?}"
        );
        assert!(
            Pairs::unit(&far, &shifted(far)).estimate(model).is_some(),
            "{model:?}"
        );
    }
    let line = |delta: f64| [DVec2::ZERO, DVec2::new(1.0, 0.0), DVec2::new(2.0, delta)];
    let target = [
        DVec2::new(5.0, 5.0),
        DVec2::new(6.0, 5.5),
        DVec2::new(7.0, 6.5),
    ];
    assert!(
        Pairs::unit(&line(3e-5), &target)
            .estimate(TransformType::Affine)
            .is_none()
    );
    assert!(
        Pairs::unit(&line(1e-3), &target)
            .estimate(TransformType::Affine)
            .is_some()
    );
}
