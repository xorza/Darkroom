//! Synthetic tests for star detection algorithms.
//!
//! These tests use generated star fields to verify detection accuracy
//! without requiring real calibration data.
//!
//! # Where a star-detection test goes
//!
//! One question decides it: **does the test call several stages together, or one function?**
//!
//! - One function — `background_map::estimate`, `gaussian_convolve`, `measure_star` — goes in that
//!   module's own `tests/`, beside the unit tests for the same code. It does not belong here
//!   however realistic its fixture is.
//! - Several stages — `detect_stars_test`, which runs thresholding, labeling and deblending as the
//!   detect stage does, or the whole `StarDetector::detect` — goes here, in
//!   [`stage_effects`] when the field is built to stress one stage's contribution, or in
//!   [`pipeline_tests`] when it grades overall detection quality on a kind of field.
//!
//! [`metric_curves`] and [`subpixel_accuracy`] are the graded forward-model tests: they assert the
//! *shape* of the detector's response through `metrics` rather than a pass/fail threshold.

mod mem_budget;
mod mem_budget_probe;
mod metric_curves;
mod pipeline_tests;
#[cfg(feature = "real-data")]
pub(crate) mod real_data;
mod stage_effects;
mod subpixel_accuracy;

use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::prelude::*;
use crate::testing::synthetic::artifacts::{add_bayer_pattern, add_cosmic_rays};
use crate::testing::synthetic::camera::{Camera, PsfModel};
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};

/// How near a detection must lie to a source to be that source's: one FWHM of the scenarios' 4-px
/// PSF, past which a centroid is not a measurement of the source.
const MATCH_RADIUS: f64 = 4.0;

/// How far a source must stand from every other one, and from the frame's edge, for the stage tests
/// to hold the detector to an exact count around it: 3 FWHM at the scenarios' 4 px, where a star
/// has fallen to 2⁻³⁶ of its peak and a neighbour can neither blend with it nor share its region.
const ISOLATION: f64 = 12.0;

/// The `points` at least [`ISOLATION`] from each other, from every one of `others`, and from the
/// edge of a `size` frame.
fn isolated(points: &[DVec2], others: &[DVec2], size: Size2us) -> Vec<DVec2> {
    let far = |a: DVec2, b: DVec2| a.distance(b) >= ISOLATION;
    let inside = |p: DVec2| {
        p.min_element() >= ISOLATION
            && p.x <= size.width as f64 - 1.0 - ISOLATION
            && p.y <= size.height as f64 - 1.0 - ISOLATION
    };
    points
        .iter()
        .enumerate()
        .filter(|&(i, &p)| {
            inside(p)
                && points.iter().enumerate().all(|(j, &q)| i == j || far(p, q))
                && others.iter().all(|&q| far(p, q))
        })
        .map(|(_, &p)| p)
        .collect()
}

/// How many of `positions` lie within `radius` of `at`.
fn near(at: DVec2, positions: &[DVec2], radius: f64) -> usize {
    positions
        .iter()
        .filter(|&&p| p.distance(at) <= radius)
        .count()
}

/// Detection config for synthetic (already-linear) frames: the CFA matched filter is disabled
/// so the measured FWHM stays accurate.
fn synthetic_config() -> Config {
    let mut config = Config::default();
    config.fwhm.mode = None;
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

        // Artifacts off the light path, applied to the pixels post-render; the truth records where
        // the cosmic rays landed.
        if self.cosmic_rays > 0 || self.bayer {
            let mut px = frame.image.channel(0).pixels().to_vec();
            if self.cosmic_rays > 0 {
                frame.truth.cosmic_rays = add_cosmic_rays(
                    &mut px,
                    self.size.width,
                    self.cosmic_rays,
                    (0.5, 1.0),
                    self.seed + 1000,
                );
            }
            if self.bayer {
                add_bayer_pattern(&mut px, self.size.width, 0.08, CfaPattern::Rggb);
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
