//! Comparison image rendering for visual tests.
//!
//! Creates annotated images showing ground truth vs detected stars.

use crate::internals::synthetic::metrics::match_catalogs;
use crate::internals::synthetic::observe::ObservedSource;
use crate::internals::visual::{ToneMap, debug_file, gray_to_rgb, save_image};
use crate::star_detection::star::Star;
use glam::Vec2;
use imaginarium::drawing::{draw_circle, draw_cross};
use imaginarium::{Buffer2, Image};

/// Colors for comparison images.
mod colors {
    use imaginarium::Color;

    pub(super) const GREEN: Color = Color::rgb(0.0, 1.0, 0.0); // Correctly detected
    pub(super) const RED: Color = Color::rgb(1.0, 0.2, 0.2); // Missed (false negative)
    pub(super) const YELLOW: Color = Color::rgb(1.0, 1.0, 0.0); // False positive
    pub(super) const CYAN: Color = Color::rgb(0.0, 1.0, 1.0); // Detected centroid
    pub(super) const MAGENTA: Color = Color::rgb(1.0, 0.0, 1.0); // True centroid
}

/// Ground truth against the stars a detection found, drawn over the plane they were found on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Comparison<'a> {
    pub(crate) ground_truth: &'a [ObservedSource],
    pub(crate) detected: &'a [Star],
    /// The greatest distance, in pixels, at which a detected star matches a true one.
    pub(crate) match_radius: f32,
}

impl Comparison<'_> {
    /// `pixels` in RGB with: green circles on the true stars detected, red circles and magenta
    /// crosses on those missed, cyan crosses on the matched detections, and yellow circles and
    /// crosses on false positives.
    fn image(&self, pixels: &Buffer2<f32>) -> Image {
        let Self {
            ground_truth,
            detected,
            match_radius,
        } = *self;
        let mut image = gray_to_rgb(pixels, ToneMap::Clamp);
        let truth_poss: Vec<glam::DVec2> = ground_truth.iter().map(|s| s.pos).collect();
        let detected_poss: Vec<glam::DVec2> = detected.iter().map(|s| s.pos).collect();
        let pairs = match_catalogs(&truth_poss, &detected_poss, f64::from(match_radius));
        let matched_truth: Vec<usize> = pairs.iter().map(|&(ti, _)| ti).collect();
        let matched_detected: Vec<usize> = pairs.iter().map(|&(_, di)| di).collect();

        for (i, truth) in ground_truth.iter().enumerate() {
            let centre = Vec2::new(truth.pos.x as f32, truth.pos.y as f32);
            let radius = (truth.fwhm * 1.5).max(5.0);
            if matched_truth.contains(&i) {
                draw_circle(&mut image, centre, radius, colors::GREEN, 1.0);
            } else {
                draw_circle(&mut image, centre, radius, colors::RED, 1.0);
                draw_cross(&mut image, centre, 3.0, colors::MAGENTA, 1.0);
            }
        }

        for (i, det) in detected.iter().enumerate() {
            let centre = Vec2::new(det.pos.x as f32, det.pos.y as f32);
            if matched_detected.contains(&i) {
                draw_cross(&mut image, centre, 3.0, colors::CYAN, 1.0);
            } else {
                let radius = (det.fwhm * 0.7).max(4.0);
                draw_circle(&mut image, centre, radius, colors::YELLOW, 1.0);
                draw_cross(&mut image, centre, 3.0, colors::YELLOW, 1.0);
            }
        }

        image
    }

    /// Write [`Self::image`] of `pixels` under `test_output/<name>`, when debug output is on.
    pub(crate) fn save(&self, pixels: &Buffer2<f32>, name: &str) {
        if debug_file(name).is_some() {
            save_image(self.image(pixels), name);
        }
    }
}
