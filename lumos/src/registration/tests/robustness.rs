//! Registration of star catalogs whose correspondence is known: every model, exact and noisy
//! positions, missing and spurious stars, partial overlap, large rotations and scales, and the
//! smallest catalogs that register.

use crate::internals::prelude::*;
use crate::internals::synthetic::transforms::{
    add_spurious_star_list, add_star_noise, generate_random_stars,
};
use crate::registration::final_fit::{FitCatalogs, FitModel};
use crate::registration::ransac::config::RansacConfig;
use crate::registration::register;
use crate::registration::spatial::KdTree;
use crate::registration::tests::helpers::{self, FWHM_NORMAL, map_stars, max_deviation};
use crate::registration::transform::{Transform, TransformModel};
use crate::registration::triangle::voting::{MatchIndices, PointMatch};
use crate::registration::{Config, RegistrationError, TransformType, estimate_and_refine};
use crate::star_detection::star::Star;

/// How far an exact fit may stray, in pixels, anywhere on the field.
///
/// On exact positions the consensus set is every true pair and the refit solves it exactly but for
/// rounding: Hartley-normalized coordinates keep the solve's condition number in the hundreds, and
/// f64 rounding of coordinates up to 3000 px is 4.5e-13 px, so the fit is good to ~1e-10 px. Seven
/// orders below what a wrong model or a lost pair leaves.
const EXACT_FIT_PX: f64 = 1e-10;

/// One registration of a synthetic catalog against a known reference → target transform.
#[derive(Debug, Clone)]
struct Scenario {
    name: &'static str,
    model: TransformType,
    truth: Transform,
    stars: usize,
    side: f64,
    seed: u64,
    /// Uniform position noise on the target, in `[−noise, noise]` px per axis.
    noise: f64,
    /// Share of the target's true stars dropped.
    missing: f64,
    /// False detections appended to the target.
    spurious: usize,
    /// Keep only target stars inside `[50, side − 50]²`, as a frame of the same size would.
    overlap: bool,
    /// Lift RANSAC's rotation and scale plausibility limits.
    unconstrained: bool,
    min_stars: usize,
    min_matches: usize,
}

/// The two catalogs, and which target star is the image of which reference star.
#[derive(Debug)]
struct Catalogs {
    reference: Vec<Star>,
    target: Vec<Star>,
    pairs: Vec<MatchIndices>,
}

impl Scenario {
    fn new(name: &'static str, model: TransformType, truth: Transform) -> Self {
        Self {
            name,
            model,
            truth,
            stars: 100,
            side: 2000.0,
            seed: 1,
            noise: 0.0,
            missing: 0.0,
            spurious: 0,
            overlap: false,
            unconstrained: false,
            min_stars: 6,
            min_matches: 4,
        }
    }

    fn catalogs(&self) -> Catalogs {
        let reference =
            generate_random_stars(self.stars, self.side, self.side, self.seed, FWHM_NORMAL);
        let images = add_star_noise(
            &map_stars(&reference, &self.truth),
            self.noise,
            self.seed + 1,
        );
        let mut drop = TestRng::new(self.seed + 2);
        let inside = |star: &Star| {
            let range = 50.0..=self.side - 50.0;
            !self.overlap || (range.contains(&star.pos.x) && range.contains(&star.pos.y))
        };
        let mut target = Vec::new();
        let mut pairs = Vec::new();
        for (index, star) in images.into_iter().enumerate() {
            if inside(&star) && drop.next_f64() >= self.missing {
                pairs.push(MatchIndices {
                    reference: index,
                    target: target.len(),
                });
                target.push(star);
            }
        }
        target.extend(add_spurious_star_list(
            &[],
            self.spurious,
            self.side,
            self.side,
            self.seed + 3,
            FWHM_NORMAL,
        ));
        Catalogs {
            reference,
            target,
            pairs,
        }
    }

    fn config(&self) -> Config {
        let mut config = Config {
            transform_type: TransformModel::Fixed(self.model),
            matching: helpers::matching_config(self.min_stars, self.min_matches),
            ..Default::default()
        };
        if self.unconstrained {
            config.ransac = RansacConfig {
                max_rotation: None,
                scale_range: None,
                ..Default::default()
            };
        }
        config
    }

    /// Register, and hold the result to the truth.
    ///
    /// The matched pairs are exactly the true ones: every star with an image, none of the spurious
    /// ones (appended after the true images, so any target index past them is a false match), and
    /// no star whose image left the frame. The noise here is at most 0.5 px per axis, inside the
    /// 3.03 px recovery radius of `FWHM_NORMAL`, so noise loses no pair either.
    ///
    /// The fit is held to [`EXACT_FIT_PX`] on exact positions. With noise of amplitude `a` — so
    /// `σ = a/√3` per axis — a least-squares fit's error at a point is `σ·√h` for the point's
    /// leverage `h`. For `n` pairs spread uniformly over a square and points inside its bounds,
    /// `h ≤ (1 + 3u² + 3v²)/n ≤ 7/n` for a model linear in the coordinates, and the homography's
    /// linearized `u², uv` terms raise that to 21/n. Five σ per axis, √2 for the distance: the fit
    /// is checked over the box the paired reference stars span, where that holds.
    fn check(&self) {
        let catalogs = self.catalogs();
        let result = register(&catalogs.reference, &catalogs.target, &self.config())
            .unwrap_or_else(|error| panic!("{}: {error}", self.name));
        let found = result.transform();
        assert_eq!(found.transform_type(), self.model, "{}", self.name);

        let mut matched: Vec<MatchIndices> =
            result.matched_stars().iter().map(|m| m.indices).collect();
        matched.sort_by_key(|m| (m.reference, m.target));
        assert_eq!(matched, catalogs.pairs, "{}: matched pairs", self.name);

        let (low, high) = catalogs.pairs.iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(low, high), pair| {
                let p = catalogs.reference[pair.reference].pos;
                (low.min(p), high.max(p))
            },
        );
        let deviation = max_deviation(&found, &self.truth, low, high);
        let bound = if self.noise == 0.0 {
            EXACT_FIT_PX
        } else {
            let leverage = if self.model == TransformType::Homography {
                21.0
            } else {
                7.0
            };
            5.0 * (self.noise / 3f64.sqrt()) * (2.0 * leverage / catalogs.pairs.len() as f64).sqrt()
        };
        assert!(
            deviation <= bound,
            "{}: the fit strays {deviation} px from the truth, bound {bound}",
            self.name
        );
        if self.noise == 0.0 {
            assert!(
                result.rms_error() <= EXACT_FIT_PX,
                "{}: rms {}",
                self.name,
                result.rms_error()
            );
        }
    }
}

/// `s·R(θ)` about `(c, c)`, then `offset`: a similarity about the field's centre.
fn about_centre(c: f64, offset: DVec2, angle_deg: f64, scale: f64) -> Transform {
    let linear = Transform::similarity(DVec2::ZERO, angle_deg.to_radians(), scale);
    let centre = DVec2::splat(c);
    Transform::similarity(
        centre + offset - linear.apply(centre),
        angle_deg.to_radians(),
        scale,
    )
}

fn run(scenarios: &[Scenario]) {
    for scenario in scenarios {
        scenario.check();
    }
}

/// Every model recovers its own transform exactly, and an over-parameterized model recovers a
/// simpler one: a similarity of Euclidean data has scale 1, an affine of similarity data is that
/// similarity.
#[test]
fn every_model_recovers_an_exact_transform() {
    use TransformType::*;
    let translation = Scenario {
        stars: 50,
        side: 1000.0,
        ..Scenario::new(
            "translation",
            Translation,
            Transform::translation(DVec2::new(25.5, -15.3)),
        )
    };
    run(&[
        translation.clone(),
        Scenario {
            stars: 60,
            seed: 7,
            ..Scenario::new(
                "large translation",
                Translation,
                Transform::translation(DVec2::new(200.0, -150.0)),
            )
        },
        Scenario::new(
            "rotation about the centre",
            Euclidean,
            about_centre(1000.0, DVec2::ZERO, 0.5, 1.0),
        ),
        Scenario::new(
            "rotation and translation",
            Euclidean,
            about_centre(1000.0, DVec2::new(35.0, 25.0), 0.5, 1.0),
        ),
        Scenario::new(
            "similarity",
            Similarity,
            about_centre(1000.0, DVec2::new(15.0, -10.0), 0.3, 1.002),
        ),
        Scenario::new(
            "differential scale",
            Affine,
            Transform::affine([1.002, 0.0, 10.0, 0.0, 0.998, -5.0]),
        ),
        Scenario::new(
            "shear",
            Affine,
            Transform::affine([1.0, 0.003, 10.0, 0.0, 1.0, -5.0]),
        ),
        Scenario::new(
            "rotation and differential scale",
            Affine,
            Transform::affine([
                1.003 * 0.4f64.to_radians().cos(),
                -0.997 * 0.4f64.to_radians().sin(),
                20.0,
                1.003 * 0.4f64.to_radians().sin(),
                0.997 * 0.4f64.to_radians().cos(),
                -12.0,
            ]),
        ),
        Scenario::new(
            "mild perspective",
            Homography,
            Transform::homography([1.0, 0.0, 25.0, 0.0, 1.0, -18.0, 1e-5, 5e-6]),
        ),
        Scenario::new(
            "perspective and rotation",
            Homography,
            Transform::homography([
                0.3f64.to_radians().cos(),
                -0.3f64.to_radians().sin(),
                30.0,
                0.3f64.to_radians().sin(),
                0.3f64.to_radians().cos(),
                -20.0,
                2e-5,
                1e-5,
            ]),
        ),
        Scenario::new(
            "similarity model on Euclidean data",
            Similarity,
            about_centre(750.0, DVec2::new(20.0, 15.0), 0.6, 1.0),
        ),
        Scenario::new(
            "affine model on similarity data",
            Affine,
            about_centre(1000.0, DVec2::new(30.0, -20.0), 0.4, 1.005),
        ),
        Scenario::new(
            "quarter-pixel translation",
            Translation,
            Transform::translation(DVec2::new(0.25, -0.25)),
        ),
        Scenario::new(
            "half-pixel translation",
            Translation,
            Transform::translation(DVec2::new(0.5, 0.5)),
        ),
        Scenario::new(
            "hundredth of a degree",
            Euclidean,
            about_centre(1000.0, DVec2::ZERO, 0.01, 1.0),
        ),
        Scenario::new(
            "scale of 1.0005",
            Similarity,
            about_centre(1000.0, DVec2::ZERO, 0.0, 1.0005),
        ),
        Scenario {
            stars: 200,
            side: 3000.0,
            min_stars: 10,
            min_matches: 8,
            ..Scenario::new(
                "dense field, large shift and scale",
                Similarity,
                about_centre(1500.0, DVec2::new(150.0, -100.0), 0.0, 1.008),
            )
        },
    ]);
}

/// A rotation or scale far from the identity registers once RANSAC's plausibility limits are
/// lifted.
#[test]
fn large_rotations_and_scales_register() {
    use TransformType::*;
    let large = |name, model, truth| Scenario {
        unconstrained: true,
        ..Scenario::new(name, model, truth)
    };
    run(&[
        large(
            "45°",
            Euclidean,
            about_centre(1000.0, DVec2::new(20.0, -15.0), 45.0, 1.0),
        ),
        large(
            "90°",
            Euclidean,
            about_centre(1000.0, DVec2::ZERO, 90.0, 1.0),
        ),
        large(
            "−45°",
            Euclidean,
            about_centre(1000.0, DVec2::new(-10.0, 25.0), -45.0, 1.0),
        ),
        large(
            "scale 2",
            Similarity,
            about_centre(1000.0, DVec2::ZERO, 0.0, 2.0),
        ),
        large(
            "scale ½",
            Similarity,
            about_centre(1000.0, DVec2::ZERO, 0.0, 0.5),
        ),
        large(
            "scale 1.5 at 30°",
            Similarity,
            about_centre(1000.0, DVec2::new(50.0, -30.0), 30.0, 1.5),
        ),
    ]);
}

/// A meridian flip turns the field by 180°, and an alt-az session turns it by any angle: under the
/// default limits, which set no rotation limit, both register exactly, for a rigid and a similarity
/// model.
#[test]
fn a_meridian_flip_registers_under_the_default_limits() {
    use TransformType::*;
    run(&[
        Scenario::new(
            "180°",
            Euclidean,
            about_centre(1000.0, DVec2::new(12.0, -7.0), 180.0, 1.0),
        ),
        Scenario::new(
            "180° at scale 1.01",
            Similarity,
            about_centre(1000.0, DVec2::new(-30.0, 4.0), 180.0, 1.01),
        ),
        Scenario::new(
            "37°",
            Euclidean,
            about_centre(1000.0, DVec2::new(5.0, 9.0), 37.0, 1.0),
        ),
    ]);
}

/// Missing and spurious stars are left out of the match, and the fit stays exact.
#[test]
fn missing_and_spurious_stars_are_left_out() {
    use TransformType::*;
    let shift = |x, y| Transform::translation(DVec2::new(x, y));
    run(&[
        Scenario {
            spurious: 10,
            seed: 11,
            ..Scenario::new("10% spurious", Translation, shift(30.0, -20.0))
        },
        Scenario {
            missing: 0.1,
            seed: 12,
            ..Scenario::new("10% missing", Translation, shift(25.0, 15.0))
        },
        Scenario {
            missing: 0.1,
            spurious: 10,
            seed: 13,
            ..Scenario::new("10% of each", Translation, shift(40.0, -30.0))
        },
        Scenario {
            stars: 80,
            spurious: 16,
            seed: 14,
            ..Scenario::new(
                "20% spurious, rotated",
                Euclidean,
                about_centre(1000.0, DVec2::new(35.0, 25.0), 0.5, 1.0),
            )
        },
        Scenario {
            spurious: 15,
            seed: 15,
            ..Scenario::new(
                "affine with spurious stars",
                Affine,
                Transform::affine([1.01, 0.005, 40.0, 0.0, 0.99, -25.0]),
            )
        },
        Scenario {
            stars: 120,
            spurious: 20,
            seed: 16,
            min_stars: 8,
            min_matches: 6,
            ..Scenario::new(
                "homography with spurious stars",
                Homography,
                Transform::homography([
                    0.3f64.to_radians().cos(),
                    -0.3f64.to_radians().sin(),
                    30.0,
                    0.3f64.to_radians().sin(),
                    0.3f64.to_radians().cos(),
                    -20.0,
                    2e-5,
                    1e-5,
                ]),
            )
        },
    ]);
}

/// A target that sees only part of the reference field: the reference catalog is passed whole, and
/// the match holds exactly the stars both frames contain.
#[test]
fn partial_overlap_matches_only_the_shared_stars() {
    use TransformType::*;
    let overlapping = |name, model, truth, seed| Scenario {
        overlap: true,
        seed,
        stars: 150,
        ..Scenario::new(name, model, truth)
    };
    run(&[
        overlapping(
            "75%",
            Translation,
            Transform::translation(DVec2::new(500.0, 0.0)),
            21,
        ),
        overlapping(
            "50%",
            Translation,
            Transform::translation(DVec2::new(1000.0, 0.0)),
            22,
        ),
        overlapping(
            "diagonal",
            Translation,
            Transform::translation(DVec2::new(400.0, 400.0)),
            23,
        ),
        overlapping(
            "rotated",
            Euclidean,
            about_centre(1000.0, DVec2::new(600.0, 0.0), 0.5, 1.0),
            24,
        ),
    ]);
}

/// Position noise moves the fit by no more than its least-squares error.
#[test]
fn noisy_positions_fit_within_their_noise() {
    use TransformType::*;
    run(&[
        Scenario {
            noise: 0.5,
            seed: 31,
            ..Scenario::new(
                "similarity, ±0.5 px",
                Similarity,
                about_centre(1000.0, DVec2::new(15.0, -10.0), 0.3, 1.002),
            )
        },
        Scenario {
            noise: 0.3,
            missing: 0.1,
            spurious: 5,
            seed: 32,
            ..Scenario::new(
                "Euclidean, ±0.3 px, 10% missing, 5 spurious",
                Euclidean,
                about_centre(1000.0, DVec2::new(45.0, -30.0), 1.0, 1.0),
            )
        },
        Scenario {
            noise: 0.5,
            overlap: true,
            stars: 150,
            seed: 33,
            ..Scenario::new(
                "Euclidean, ±0.5 px, 60% overlap",
                Euclidean,
                about_centre(1000.0, DVec2::new(800.0, 0.0), 0.5, 1.0),
            )
        },
        Scenario {
            noise: 0.4,
            missing: 0.1,
            seed: 34,
            ..Scenario::new(
                "affine, ±0.4 px, 10% missing",
                Affine,
                Transform::affine([1.005, 0.003, 30.0, 0.0, 0.995, 20.0]),
            )
        },
        Scenario {
            noise: 0.3,
            overlap: true,
            stars: 150,
            seed: 35,
            min_stars: 8,
            min_matches: 6,
            ..Scenario::new(
                "homography, ±0.3 px, 85% overlap",
                Homography,
                Transform::homography([1.0, 0.002, 300.0, -0.001, 1.0, -20.0, 2e-5, 1e-5]),
            )
        },
    ]);
}

/// The smallest catalogs that register: six stars for a translation and eight for a similarity,
/// at `min_stars` 4 and `min_matches` 3 — and three stars are refused before any matching.
#[test]
fn the_smallest_catalogs_register_and_three_stars_do_not() {
    let small = |name, model, truth, stars| Scenario {
        stars,
        side: 1000.0,
        min_stars: 4,
        min_matches: 3,
        ..Scenario::new(name, model, truth)
    };
    run(&[
        small(
            "six stars",
            TransformType::Translation,
            Transform::translation(DVec2::new(15.0, -10.0)),
            6,
        ),
        small(
            "eight stars",
            TransformType::Similarity,
            about_centre(500.0, DVec2::new(10.0, -8.0), 0.5, 1.01),
            8,
        ),
    ]);

    let three = small(
        "three stars",
        TransformType::Translation,
        Transform::translation(DVec2::new(10.0, 5.0)),
        3,
    );
    let catalogs = three.catalogs();
    let result = register(&catalogs.reference, &catalogs.target, &three.config());
    assert!(
        matches!(
            result,
            Err(RegistrationError::InsufficientStars { found: 3, .. })
        ),
        "expected InsufficientStars {{ found: 3 }}, got {result:?}"
    );
}

/// A fit is held to `min_matches` after the final fit, not only the matcher. Twelve matches of
/// which three agree on a translation: RANSAC fits the three exactly, the RMS is zero, and the
/// final fit has no star within reach of the others — so the fit rests on 3 pairs where 8 are
/// asked for.
#[test]
fn a_fit_on_fewer_inliers_than_min_matches_is_refused() {
    let shift = DVec2::new(4.0, -2.0);
    let reference: Vec<DVec2> = (0..12u8)
        .map(|i| DVec2::new(40.0 * f64::from(i % 4), 40.0 * f64::from(i / 4)))
        .collect();
    let target: Vec<DVec2> = reference
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            if i < 3 {
                p + shift
            } else {
                DVec2::new(
                    900.0 + 37.0 * f64::from(i as u8),
                    700.0 - 53.0 * f64::from(i as u8),
                )
            }
        })
        .collect();
    let matches: Vec<PointMatch> = (0..12)
        .map(|i| PointMatch {
            indices: MatchIndices {
                reference: i,
                target: i,
            },
            confidence: 1.0,
        })
        .collect();
    let mut config = Config {
        transform_type: TransformModel::Fixed(TransformType::Translation),
        ..Config::default()
    };
    config.ransac.seed = 1;
    let as_stars = |points: &[DVec2]| points.iter().map(|&p| Star::at(p)).collect::<Vec<_>>();
    let catalogs = FitCatalogs::new(&as_stars(&reference), &as_stars(&target)).unwrap();
    let error = estimate_and_refine(
        &reference,
        &KdTree::build(target).unwrap(),
        &matches,
        &catalogs,
        FitModel {
            transform: TransformType::Translation,
            sip: None,
        },
        1.0,
        &config,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            RegistrationError::TooFewInliers {
                found: 3,
                required: 8
            }
        ),
        "{error:?}"
    );
}
