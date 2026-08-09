//! Comparison image rendering for visual tests.
//!
//! Creates annotated images showing ground truth vs detected stars.

use crate::math::size2us::Size2us;
use crate::stacking::star_detection::star::Star;
use crate::testing::synthetic::metrics::match_catalogs;
use crate::testing::synthetic::observe::ObservedSource;
use crate::testing::visual::{ToneMap, gray_to_rgb};
use glam::Vec2;
use imaginarium::Image;
use imaginarium::drawing::{draw_circle, draw_cross};

/// Colors for comparison images.
mod colors {
    use imaginarium::Color;

    pub(super) const GREEN: Color = Color::rgb(0.0, 1.0, 0.0); // Correctly detected
    pub(super) const RED: Color = Color::rgb(1.0, 0.2, 0.2); // Missed (false negative)
    pub(super) const YELLOW: Color = Color::rgb(1.0, 1.0, 0.0); // False positive
    pub(super) const CYAN: Color = Color::rgb(0.0, 1.0, 1.0); // Detected centroid
    pub(super) const MAGENTA: Color = Color::rgb(1.0, 0.0, 1.0); // True centroid
}

/// Create a comparison image showing ground truth and detected stars.
///
/// # Arguments
/// * `pixels` - Background image pixels
/// * `size` - Image dimensions
/// * `ground_truth` - True star positions
/// * `detected` - Detected stars
/// * `match_radius` - Maximum distance for matching (in pixels)
///
/// # Returns
/// RGB image with:
/// - Blue circles: ground truth positions
/// - Green circles: correctly detected
/// - Red circles: missed stars
/// - Yellow circles: false positives
/// - Cyan crosses: detected centroids
pub(super) fn create_comparison_image(
    pixels: &[f32],
    size: Size2us,
    ground_truth: &[ObservedSource],
    detected: &[Star],
    match_radius: f32,
) -> Image {
    let mut image = gray_to_rgb(pixels, size, ToneMap::Clamp);

    // Match detected stars to ground truth
    let truth_positions: Vec<glam::DVec2> = ground_truth.iter().map(|s| s.pos).collect();
    let detected_positions: Vec<glam::DVec2> = detected.iter().map(|s| s.pos).collect();
    let pairs = match_catalogs(&truth_positions, &detected_positions, match_radius as f64);
    let matched_truth: Vec<usize> = pairs.iter().map(|&(ti, _)| ti).collect();
    let matched_detected: Vec<usize> = pairs.iter().map(|&(_, di)| di).collect();

    // Draw ground truth stars
    for (i, truth) in ground_truth.iter().enumerate() {
        let cx = truth.pos.x as f32;
        let cy = truth.pos.y as f32;
        let radius = (truth.fwhm * 1.5).max(5.0);

        // Color depends on whether it was detected
        let color = if matched_truth.contains(&i) {
            colors::GREEN // Detected
        } else {
            colors::RED // Missed
        };

        draw_circle(&mut image, Vec2::new(cx, cy), radius, color, 1.0);

        // Draw true centroid position
        if !matched_truth.contains(&i) {
            draw_cross(&mut image, Vec2::new(cx, cy), 3.0, colors::MAGENTA, 1.0);
        }
    }

    // Draw detected stars
    for (i, det) in detected.iter().enumerate() {
        let cx = det.pos.x as f32;
        let cy = det.pos.y as f32;

        if matched_detected.contains(&i) {
            // True positive - draw centroid cross
            draw_cross(&mut image, Vec2::new(cx, cy), 3.0, colors::CYAN, 1.0);
        } else {
            // False positive - draw yellow circle
            let radius = (det.fwhm * 0.7).max(4.0);
            draw_circle(&mut image, Vec2::new(cx, cy), radius, colors::YELLOW, 1.0);
            draw_cross(&mut image, Vec2::new(cx, cy), 3.0, colors::YELLOW, 1.0);
        }
    }

    image
}
