//! Full pipeline tests - tests complete star detection on various scenarios.

use crate::AstroImage;
use crate::astro_image::ImageDimensions;
use crate::star_detection::config::Config;
use crate::star_detection::detector::StarDetector;
use crate::star_detection::tests::common::output::image_writer::{save_comparison, save_grayscale};
use crate::star_detection::tests::common::output::metrics::{
    DetectionMetrics, compute_detection_metrics, save_metrics,
};
use crate::testing::synthetic::artifacts::{BayerPattern, add_bayer_pattern, add_cosmic_rays};
use crate::testing::synthetic::camera::{Camera, PsfModel};
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use common::test_utils::test_output_path;

mod challenging_tests;
mod standard_tests;

/// Source placement for a detection scenario.
#[derive(Debug, Clone, Copy)]
enum Placement {
    Uniform { margin: f64 },
    Cluster,
}

/// A compact forward-model detection scenario. Defaults to a clean, brightly-detected uniform
/// field; override fields per test. `frame()` renders it (applying cosmic-ray / Bayer
/// artifacts via the kept primitives) into a `SimFrame` for `run_test`.
#[derive(Debug, Clone)]
struct Scenario {
    width: usize,
    height: usize,
    num_stars: usize,
    /// Log-uniform total-flux range; higher = brighter / easier to detect.
    flux: (f32, f32),
    fwhm: f32,
    psf: Option<PsfModel>,
    background: BackgroundField,
    /// Sensor full well (electrons) — lower deepens shot noise.
    full_well_e: f32,
    read_noise_e: f32,
    placement: Placement,
    cosmic_rays: usize,
    bayer: bool,
    seed: u64,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            num_stars: 30,
            // Bright but below the well: a flux-14 star peaks ~0.6 (fwhm 4), clear of the
            // detector's saturation cut so every star is detectable.
            flux: (5.0, 14.0),
            fwhm: 4.0,
            psf: None,
            background: BackgroundField::Uniform { level: 0.1 },
            full_well_e: 50_000.0,
            read_noise_e: 3.0,
            placement: Placement::Uniform { margin: 16.0 },
            cosmic_rays: 0,
            bayer: false,
            seed: 42,
        }
    }
}

impl Scenario {
    fn frame(&self) -> SimFrame {
        let scene = match self.placement {
            Placement::Uniform { margin } => Scene::random_field(
                self.width,
                self.height,
                self.num_stars,
                self.flux,
                self.background.clone(),
                margin,
                self.seed,
            ),
            Placement::Cluster => Scene::cluster(
                self.width,
                self.height,
                self.num_stars,
                self.flux,
                self.background.clone(),
                self.seed,
            ),
        };
        let camera = Camera {
            psf: self.psf.unwrap_or(PsfModel::Gaussian { fwhm: self.fwhm }),
            full_well_e: self.full_well_e,
            read_noise_e: self.read_noise_e,
            ..Camera::realistic(self.fwhm)
        };
        let mut frame = render(&scene, &camera, &Observation::reference(self.seed));

        // Artifacts that don't follow the light path: applied to the pixels post-render,
        // truth (positions/flux/fwhm) unchanged.
        if self.cosmic_rays > 0 || self.bayer {
            let mut px = frame.image.channel(0).pixels().to_vec();
            if self.cosmic_rays > 0 {
                add_cosmic_rays(
                    &mut px,
                    self.width,
                    self.cosmic_rays,
                    (0.5, 1.0),
                    self.seed + 1000,
                );
            }
            if self.bayer {
                add_bayer_pattern(&mut px, self.width, 0.08, BayerPattern::RGGB);
            }
            for p in &mut px {
                *p = p.clamp(0.0, 1.0);
            }
            frame.image = AstroImage::from_planar_channels(
                ImageDimensions::new((self.width, self.height), 1),
                [px],
            );
        }
        frame
    }
}

/// Run full detection on a pre-rendered forward-model frame and return metrics.
///
/// Runs detection on `frame.image`, grades it against `frame.truth.sources`, saves
/// input/comparison/metrics files, and returns the metrics for the caller to validate.
fn run_test(
    name: &str,
    prefix: &str,
    frame: &SimFrame,
    detection_config: &Config,
) -> DetectionMetrics {
    let width = frame.image.width();
    let height = frame.image.height();
    let pixels = frame.image.channel(0).pixels();
    let truth = &frame.truth.sources;

    save_grayscale(
        pixels,
        width,
        height,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_input.png",
            prefix, name
        )),
    );

    let mut detector = StarDetector::from_config(detection_config.clone());
    let stars = detector.detect(&frame.image).stars;

    // All forward-model sources share the instrument PSF; match within ~2 FWHM.
    let match_radius = truth.first().map_or(8.0, |s| s.fwhm) * 2.0;
    let metrics = compute_detection_metrics(truth, &stars, match_radius);

    save_comparison(
        pixels,
        width,
        height,
        truth,
        &stars,
        match_radius,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_comparison.png",
            prefix, name
        )),
    );

    save_metrics(
        &metrics,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_metrics.txt",
            prefix, name
        )),
    );

    println!("\n{} results:", name);
    println!("{}", metrics);

    metrics
}
