//! Synthetic tests for star detection algorithms.
//!
//! These tests use generated star fields to verify detection accuracy
//! without requiring real calibration data.

mod metric_curves;
mod pipeline_tests;
mod stage_tests;
mod subpixel_accuracy;

use crate::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::synthetic::artifacts::{BayerPattern, add_bayer_pattern, add_cosmic_rays};
use crate::testing::synthetic::camera::{Camera, PsfModel};
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use glam::DVec2;

/// Detection config for synthetic (already-linear) frames: the CFA matched filter is disabled
/// so the measured FWHM stays accurate.
fn synthetic_config() -> Config {
    let mut config = Config::default();
    config.fwhm.expected = 0.0;
    config.filter.min_snr = 5.0;
    config
}

/// True source positions of a rendered frame.
fn truth_positions(frame: &SimFrame) -> Vec<DVec2> {
    frame.truth.sources.iter().map(|s| s.pos).collect()
}

/// Detect on `frame.image` with `config` and return the detected star positions.
fn detected_positions(frame: &SimFrame, config: &Config) -> Vec<DVec2> {
    StarDetector::from_config(config.clone())
        .unwrap()
        .detect(&frame.image)
        .stars
        .iter()
        .map(|s| s.pos)
        .collect()
}

/// Source placement for a forward-model detection scenario.
#[derive(Debug, Clone, Copy)]
pub(super) enum Placement {
    Uniform { margin: f64 },
    Cluster,
}

/// A compact forward-model detection scenario shared by the pipeline and stage tests.
///
/// Defaults to a clean, brightly-detected uniform field; override fields per test. `frame()`
/// renders it (applying cosmic-ray / Bayer artifacts via the kept primitives) into a
/// `SimFrame` whose `image` + `truth.sources` the tests grade against.
#[derive(Debug, Clone)]
pub(super) struct Scenario {
    pub(super) size: Size2us,
    pub(super) num_stars: usize,
    /// Log-uniform total-flux range; higher = brighter / easier to detect.
    pub(super) flux: (f32, f32),
    pub(super) fwhm: f32,
    pub(super) psf: Option<PsfModel>,
    pub(super) background: BackgroundField,
    /// Sensor full well (electrons) — lower deepens shot noise.
    pub(super) full_well_e: f32,
    pub(super) read_noise_e: f32,
    pub(super) placement: Placement,
    pub(super) cosmic_rays: usize,
    pub(super) bayer: bool,
    pub(super) seed: u64,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            size: Size2us::new(256, 256),
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
    pub(super) fn frame(&self) -> SimFrame {
        let scene = match self.placement {
            Placement::Uniform { margin } => Scene::random_field(
                self.size,
                self.num_stars,
                self.flux,
                self.background.clone(),
                margin,
                self.seed,
            ),
            Placement::Cluster => Scene::cluster(
                self.size,
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
                    self.size.width,
                    self.cosmic_rays,
                    (0.5, 1.0),
                    self.seed + 1000,
                );
            }
            if self.bayer {
                add_bayer_pattern(&mut px, self.size.width, 0.08, BayerPattern::RGGB);
            }
            for p in &mut px {
                *p = p.clamp(0.0, 1.0);
            }
            frame.image =
                LinearImage::from_planar_channels(ImageDimensions::new(self.size, 1), [px]);
        }
        frame
    }
}
