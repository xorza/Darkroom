//! Registration end to end on rendered images: one scene seen twice through the camera model, the
//! second time under a known transform, detected, registered, and graded against the transform the
//! render used.

use crate::stacking::registration::tests::helpers::{self, max_deviation, register};
use crate::stacking::registration::transform::{Transform, TransformModel};
use crate::stacking::registration::{Config, TransformType};
use crate::stacking::star_detection::config::Config as DetConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::star::Star;
use crate::testing::prelude::*;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::metrics;
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};

/// A scene of `stars` over `size`, its reference frame and the frame `truth` maps it to.
#[derive(Debug)]
struct Rendered {
    name: &'static str,
    size: Size2us,
    stars: usize,
    seed: u64,
    truth: Transform,
    model: TransformType,
    camera: Camera,
    min_matches: usize,
}

impl Rendered {
    fn new(name: &'static str, truth: Transform, model: TransformType) -> Self {
        Self {
            name,
            size: Size2us::new(256, 256),
            stars: 60,
            seed: 1,
            truth,
            model,
            camera: Camera::realistic(4.0),
            min_matches: 4,
        }
    }

    /// Detect, register, and hold the fit to the truth.
    ///
    /// The fit's error comes from the centroids, measured here against the render's own truth: `σ`
    /// per axis over both frames' detections. A pair's offset then carries `σ√2` per axis, and a
    /// least-squares fit on `n` pairs spread over the frame errs at a point by that times `√h`,
    /// with leverage `h ≤ 7/n` inside the box the stars span (see `robustness::Scenario::check`).
    /// Five of those, √2 for the distance. Every matched pair is a true one: its offset from the
    /// truth is within the same five σ.
    fn check(&self) {
        let scene = Scene::random_field(
            self.size,
            self.stars,
            (6.0, 16.0),
            BackgroundField::Uniform { level: 0.1 },
            16.0,
            self.seed,
        );
        let reference = render(&scene, &self.camera, &Observation::reference(self.seed));
        let target = render(
            &scene,
            &self.camera,
            &Observation {
                transform: self.truth,
                ..Observation::reference(self.seed + 1)
            },
        );
        let mut detector = detector();
        let reference_stars = detector.detect(&reference.image).stars;
        let target_stars = detector.detect(&target.image).stars;

        let config = Config {
            transform_type: TransformModel::Fixed(self.model),
            matching: helpers::matching_config(6, self.min_matches),
            ..Default::default()
        };
        let result = register(&reference_stars, &target_stars, &config)
            .unwrap_or_else(|error| panic!("{}: {error}", self.name));

        let sigma = centroid_sigma(&[(&reference, &reference_stars), (&target, &target_stars)]);
        let n = result.num_inliers() as f64;
        let pair_sigma = sigma * 2f64.sqrt();
        let bound = 5.0 * pair_sigma * (2.0 * 7.0 / n).sqrt();
        let (low, high) = result.matched_stars().iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(low, high), m| {
                let p = reference_stars[m.indices.reference].pos;
                (low.min(p), high.max(p))
            },
        );
        let deviation = max_deviation(&result.transform(), &self.truth, low, high);
        assert!(
            deviation <= bound,
            "{}: the fit strays {deviation} px from the truth over {n} pairs, bound {bound} (σ {sigma})",
            self.name
        );
        for m in result.matched_stars() {
            let r = reference_stars[m.indices.reference].pos;
            let t = target_stars[m.indices.target].pos;
            assert!(
                self.truth.apply(r).distance(t) <= 5.0 * pair_sigma * 2f64.sqrt(),
                "{}: pair {:?} is not a true one",
                self.name,
                m.indices
            );
        }
    }
}

/// A detector for the synthetic frames: no FWHM prior, a 5 SNR floor and a 3σ threshold.
fn detector() -> StarDetector {
    let mut config = DetConfig::default();
    config.fwhm.expected = 0.0;
    config.filter.min_snr = 5.0;
    config.detection.sigma_threshold = 3.0;
    StarDetector::from_config(config).unwrap()
}

/// The RMS centroid error per axis of `frames`' detections against their renders' truth, over the
/// detections within 1.5 px of a true source.
fn centroid_sigma(frames: &[(&SimFrame, &Vec<Star>)]) -> f64 {
    let mut sum = 0.0;
    let mut count = 0;
    for (frame, stars) in frames {
        let truth: Vec<DVec2> = frame.truth.sources.iter().map(|s| s.pos).collect();
        let found: Vec<DVec2> = stars.iter().map(|s| s.pos).collect();
        for (t, f) in metrics::match_catalogs(&truth, &found, 1.5) {
            sum += truth[t].distance_squared(found[f]);
            count += 2;
        }
    }
    assert!(count > 0, "no detection matched a source");
    (sum / f64::from(count)).sqrt()
}

#[test]
fn rendered_frames_register_to_the_transform_they_were_rendered_under() {
    use TransformType::*;
    let similarity = |dx, dy, angle_deg: f64, scale| {
        Transform::similarity(DVec2::new(dx, dy), angle_deg.to_radians(), scale)
    };
    let cases = [
        Rendered {
            stars: 50,
            seed: 42,
            ..Rendered::new(
                "translation",
                Transform::translation(DVec2::new(15.5, -12.3)),
                Translation,
            )
        },
        Rendered {
            seed: 123,
            ..Rendered::new("rotation", similarity(5.0, -3.0, 1.0, 1.0), Euclidean)
        },
        Rendered {
            stars: 70,
            seed: 456,
            ..Rendered::new("similarity", similarity(8.0, -6.0, 0.8, 1.005), Similarity)
        },
        Rendered {
            stars: 80,
            seed: 789,
            camera: Camera {
                full_well_e: 3000.0,
                read_noise_e: 15.0,
                ..Camera::realistic(4.0)
            },
            ..Rendered::new(
                "noisy camera",
                Transform::translation(DVec2::new(20.0, -15.0)),
                Translation,
            )
        },
        Rendered {
            stars: 200,
            seed: 999,
            min_matches: 8,
            ..Rendered::new("dense field", similarity(10.0, 8.0, 0.5, 1.0), Euclidean)
        },
        Rendered {
            size: Size2us::new(1024, 1024),
            stars: 100,
            seed: 111,
            ..Rendered::new(
                "large image",
                Transform::translation(DVec2::new(50.0, -35.0)),
                Translation,
            )
        },
    ];
    for case in &cases {
        case.check();
    }
}
