//! Match recovery after an initial transform estimate.

use crate::stacking::registration::ransac::transforms::estimate_transform;
use crate::stacking::registration::transform::Transform;
use crate::stacking::registration::triangle::voting::MatchIndices;
use crate::stacking::registration::*;
use crate::testing::synthetic::transforms::generate_random_positions;

/// A `angle_deg` rotation about (1000, 1000), then `offset`.
fn rotation_about_centre(angle_deg: f64, offset: DVec2) -> Transform {
    Transform::translation(offset).compose(&Transform::rotation_around(
        DVec2::splat(1000.0),
        angle_deg.to_radians(),
    ))
}

fn identity_matches(count: usize) -> Vec<MatchIndices> {
    (0..count)
        .map(|index| MatchIndices {
            reference: index,
            target: index,
        })
        .collect()
}

/// A seed transform that misses some stars is improved on: 50 stars under a 1° rotation about
/// (1000, 1000) and a shift, seeded with that rotation 0.15° short and 1 px off. The seed's error
/// grows with distance from the centre — `0.0026·r` — so it misses stars beyond the 3 px threshold
/// and finds the rest. Refitting on those finds the truth exactly, which then reaches every star:
/// all 50 come back, and the transform is the truth to rounding.
#[test]
fn recovery_improves_on_a_seed_that_misses_stars() {
    let reference = generate_random_positions(50, 2000.0, 2000.0, 42);
    let truth = rotation_about_centre(1.0, DVec2::new(30.0, -20.0));
    let target: Vec<DVec2> = reference.iter().map(|&p| truth.apply(p)).collect();
    let seed = rotation_about_centre(0.85, DVec2::new(31.0, -20.0));
    let threshold = 3.0;
    let missed = reference
        .iter()
        .zip(&target)
        .filter(|&(&r, &t)| seed.apply(r).distance(t) > threshold)
        .count();
    assert!(missed > 0, "the seed has to miss stars to test anything");

    let recovered = recover_matches(
        &reference,
        &KdTree::build(target).unwrap(),
        &seed,
        &identity_matches(3),
        threshold,
        TransformType::Euclidean,
    );
    let mut matches = recovered.matches;
    matches.sort_unstable_by_key(|m| m.reference);
    assert_eq!(matches, identity_matches(50));
    // An exact fit of exact pairs: rounding of coordinates up to 2000 px, through a
    // Hartley-normalized solve.
    for p in [DVec2::ZERO, DVec2::new(2000.0, 0.0), DVec2::splat(2000.0)] {
        assert!(recovered.transform.apply(p).distance(truth.apply(p)) < 1e-9);
    }
}

/// A run of passes that ends with fewer matches than it was given hands back the matches it was
/// given, and their fit. Ten seeds under the identity, six of whose targets sit 2 px to the right:
/// a 1 px threshold keeps the other four, whose fit — the identity — keeps them, and nothing
/// else is in reach. Four is fewer than ten, so the ten stand, with their least-squares
/// translation, `6·2/10` = 1.2 px.
#[test]
fn recovery_that_ends_with_fewer_matches_keeps_the_seed() {
    let reference: Vec<DVec2> = (0..10)
        .map(|i| DVec2::new(100.0 * f64::from(i), 50.0 * f64::from(i % 3)))
        .collect();
    let target: Vec<DVec2> = reference
        .iter()
        .enumerate()
        .map(|(i, &p)| if i < 4 { p } else { p + DVec2::new(2.0, 0.0) })
        .collect();
    let seeds = identity_matches(10);
    let recovered = recover_matches(
        &reference,
        &KdTree::build(target).unwrap(),
        &Transform::identity(),
        &seeds,
        1.0,
        TransformType::Translation,
    );
    assert_eq!(recovered.matches, seeds);
    let shift = recovered.transform.translation_components();
    assert!(
        (shift.x - 1.2).abs() < 1e-12 && shift.y.abs() < 1e-12,
        "{shift:?}"
    );
}

/// Wrong seed matches are dropped and their stars matched correctly. 30 stars under an exact
/// translation, seeded with the identity pairs of stars 0..8 and the wrong pairs 8 → 15, 9 → 20.
/// The first pass adds every other star but 15 and 20, whose targets the wrong pairs hold, and
/// drops the wrong pairs, which miss by far more than the threshold. The second pass, those
/// targets free, adds 8, 9, 15 and 20: exactly the 30 identity pairs.
#[test]
fn recovery_replaces_wrong_seed_matches() {
    let reference = generate_random_positions(30, 1000.0, 1000.0, 55);
    let shift = DVec2::new(20.0, -15.0);
    let target: Vec<DVec2> = reference.iter().map(|&p| p + shift).collect();
    let mut seeds = identity_matches(8);
    seeds.extend([
        MatchIndices {
            reference: 8,
            target: 15,
        },
        MatchIndices {
            reference: 9,
            target: 20,
        },
    ]);

    let recovered = recover_matches(
        &reference,
        &KdTree::build(target).unwrap(),
        &Transform::translation(shift),
        &seeds,
        3.0,
        TransformType::Translation,
    );
    let mut matches = recovered.matches;
    matches.sort_unstable_by_key(|m| m.reference);
    assert_eq!(matches, identity_matches(30));
}

/// A pass that drops one match and adds another leaves the count unchanged but the set changed.
/// The recovered transform must still be the fit of exactly the matches returned.
///
/// Stars 0..9 on a line; targets exist only for stars 0..5 and 9, at the reference position,
/// except star 4's, which sits 0.2 px left. The seed is the translation (0.9, 0) over stars 0..5,
/// with a 1 px threshold. Under it star 4 misses by 1.1 and is dropped while star 9 (0.9 away) is
/// added: six matches before and after. Refitting on {0, 1, 2, 3, 5, 9} gives the identity, which
/// takes star 4 back; the fit over all seven is the translation (−0.2/7, 0), under which every one
/// is within 0.2 px, so the set is stable and that fit is what comes back.
#[test]
fn a_pass_that_trades_one_match_for_another_still_refits() {
    let reference: Vec<DVec2> = (0..10)
        .map(|i| DVec2::new(10.0 * f64::from(i), 5.0))
        .collect();
    let mut target: Vec<DVec2> = vec![
        reference[0],
        reference[1],
        reference[2],
        reference[3],
        reference[4] - DVec2::new(0.2, 0.0),
        reference[5],
        reference[9],
    ];
    // Stars 6..8 have no target anywhere near them.
    target.extend([DVec2::new(500.0, 500.0), DVec2::new(600.0, 600.0)]);
    let seed = Transform::translation(DVec2::new(0.9, 0.0));
    let seed_matches = identity_matches(6);

    let recovered = recover_matches(
        &reference,
        &KdTree::build(target.clone()).unwrap(),
        &seed,
        &seed_matches,
        1.0,
        TransformType::Translation,
    );

    let mut matches: Vec<(usize, usize)> = recovered
        .matches
        .iter()
        .map(|m| (m.reference, m.target))
        .collect();
    matches.sort_unstable();
    assert_eq!(
        matches,
        [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (9, 6)]
    );

    let (fit_ref, fit_target): (Vec<DVec2>, Vec<DVec2>) = matches
        .iter()
        .map(|&(r, t)| (reference[r], target[t]))
        .unzip();
    let fitted = estimate_transform(&fit_ref, &fit_target, TransformType::Translation).unwrap();
    assert_eq!(recovered.transform.matrix(), fitted.matrix());
    assert!((recovered.transform.translation_components().x - (-0.2 / 7.0)).abs() < 1e-12);
}
