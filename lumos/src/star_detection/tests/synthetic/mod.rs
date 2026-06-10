//! Synthetic tests for star detection algorithms.
//!
//! These tests use generated star fields to verify detection accuracy
//! without requiring real calibration data.

mod debug_steps;
mod metric_curves;
mod pipeline_tests;
mod stage_tests;
mod star_field;
mod subpixel_accuracy;

use crate::star_detection::config::Config;
use crate::star_detection::detector::StarDetector;
use crate::star_detection::tests::common::output::image_writer::to_gray_image;
use crate::star_detection::tests::common::output::image_writer::{
    gray_to_rgb_image_stretched, save_image,
};
use crate::testing::init_tracing;
use crate::testing::synthetic::artifacts::{BayerPattern, add_bayer_pattern, add_cosmic_rays};
use crate::testing::synthetic::camera::{Camera, PsfModel};
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use crate::{AstroImage, ImageDimensions};
use glam::Vec2;
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};
use star_field::{SyntheticFieldConfig, SyntheticStar, generate_star_field};

/// Source placement for a forward-model detection scenario.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Placement {
    Uniform { margin: f64 },
    Cluster,
}

/// A compact forward-model detection scenario shared by the pipeline and stage tests.
///
/// Defaults to a clean, brightly-detected uniform field; override fields per test. `frame()`
/// renders it (applying cosmic-ray / Bayer artifacts via the kept primitives) into a
/// `SimFrame` whose `image` + `truth.sources` the tests grade against.
#[derive(Debug, Clone)]
pub(crate) struct Scenario {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) num_stars: usize,
    /// Log-uniform total-flux range; higher = brighter / easier to detect.
    pub(crate) flux: (f32, f32),
    pub(crate) fwhm: f32,
    pub(crate) psf: Option<PsfModel>,
    pub(crate) background: BackgroundField,
    /// Sensor full well (electrons) — lower deepens shot noise.
    pub(crate) full_well_e: f32,
    pub(crate) read_noise_e: f32,
    pub(crate) placement: Placement,
    pub(crate) cosmic_rays: usize,
    pub(crate) bayer: bool,
    pub(crate) seed: u64,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            num_stars: 30,
            // A flux-14 star peaks ~0.6 (fwhm 4): bright but clear of the saturation cut.
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
    pub(crate) fn frame(&self) -> SimFrame {
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

        // Artifacts off the light path: applied to the pixels post-render, truth unchanged.
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

#[test]
fn test_synthetic_star_detection() {
    init_tracing();

    let config = SyntheticFieldConfig {
        width: 256,
        height: 256,
        background: 0.1,
        noise_sigma: 0.02,
    };

    let true_stars = vec![
        SyntheticStar::new(64.0, 64.0, 0.8, 3.0),
        SyntheticStar::new(192.0, 64.0, 0.6, 2.5),
        SyntheticStar::new(64.0, 192.0, 0.4, 2.0),
        SyntheticStar::new(192.0, 192.0, 0.7, 3.5),
        SyntheticStar::new(128.0, 128.0, 0.5, 2.0),
    ];

    println!("Generating synthetic star field...");
    println!("  Image size: {}x{}", config.width, config.height);
    println!("  Background: {}", config.background);
    println!("  Noise sigma: {}", config.noise_sigma);
    println!("  Number of stars: {}", true_stars.len());

    for (i, star) in true_stars.iter().enumerate() {
        println!(
            "  Star {}: pos=({:.1}, {:.1}) brightness={:.2} sigma={:.1} fwhm={:.1}",
            i + 1,
            star.pos.x,
            star.pos.y,
            star.brightness,
            star.sigma,
            star.fwhm()
        );
    }

    let pixels = generate_star_field(&config, &true_stars);

    let input_image = to_gray_image(&pixels, config.width, config.height);
    let input_path =
        common::test_utils::test_output_path("synthetic_starfield/synthetic_input.png");
    input_image.save(&input_path).unwrap();
    println!("\nSaved input image to: {:?}", input_path);

    let detection_config = Config {
        min_area: 5,
        max_area: 500,
        min_snr: 20.0,
        sigma_threshold: 3.0,
        ..Default::default()
    };

    let image = AstroImage::from_pixels(
        ImageDimensions::new((config.width, config.height), 1),
        pixels.clone(),
    );
    let mut detector = StarDetector::from_config(detection_config);
    let result = detector.detect(&image);
    let detected_stars = result.stars;
    println!("\nDetected {} stars", detected_stars.len());

    for (i, star) in detected_stars.iter().enumerate() {
        println!(
            "  Detected {}: pos=({:.1}, {:.1}) flux={:.2} fwhm={:.1} snr={:.1}",
            i + 1,
            star.pos.x,
            star.pos.y,
            star.flux,
            star.fwhm,
            star.snr
        );
    }

    let mut output_image = gray_to_rgb_image_stretched(&pixels, config.width, config.height);

    let blue = Color::rgb(0.0, 0.4, 1.0);
    for star in &true_stars {
        draw_circle(
            &mut output_image,
            Vec2::new(star.pos.x, star.pos.y),
            star.fwhm() * 1.5,
            blue,
            1.0,
        );
    }

    let green = Color::GREEN;
    for star in &detected_stars {
        draw_cross(
            &mut output_image,
            Vec2::new(star.pos.x as f32, star.pos.y as f32),
            3.0,
            green,
            1.0,
        );
        draw_circle(
            &mut output_image,
            Vec2::new(star.pos.x as f32, star.pos.y as f32),
            (star.fwhm * 0.5).max(3.0),
            green,
            1.0,
        );
    }

    let output_path =
        common::test_utils::test_output_path("synthetic_starfield/synthetic_detection.png");
    save_image(output_image, &output_path);
    println!("\nSaved detection result to: {:?}", output_path);

    let mut matched = 0;
    for true_star in &true_stars {
        let closest = detected_stars.iter().min_by(|a, b| {
            let da = (a.pos.x as f32 - true_star.pos.x).powi(2)
                + (a.pos.y as f32 - true_star.pos.y).powi(2);
            let db = (b.pos.x as f32 - true_star.pos.x).powi(2)
                + (b.pos.y as f32 - true_star.pos.y).powi(2);
            da.partial_cmp(&db).unwrap()
        });

        if let Some(det) = closest {
            let dist = ((det.pos.x as f32 - true_star.pos.x).powi(2)
                + (det.pos.y as f32 - true_star.pos.y).powi(2))
            .sqrt();
            if dist < 3.0 {
                matched += 1;
            }
        }
    }

    assert_eq!(
        matched,
        true_stars.len(),
        "Should detect all {} synthetic stars",
        true_stars.len()
    );
}
