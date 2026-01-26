//! Comparison image rendering for visual tests.
//!
//! Creates annotated images showing ground truth vs detected stars.

use super::image_writer::gray_to_rgb_image_stretched;
use crate::star_detection::Star;
use crate::testing::synthetic::GroundTruthStar;
use imaginarium::drawing::{draw_circle, draw_cross, draw_line};
use imaginarium::{Color, Image};

/// Colors for comparison images.
pub mod colors {
    use imaginarium::Color;

    pub const BLUE: Color = Color::rgb(0.2, 0.4, 1.0); // Ground truth
    pub const GREEN: Color = Color::rgb(0.0, 1.0, 0.0); // Correctly detected
    pub const RED: Color = Color::rgb(1.0, 0.2, 0.2); // Missed (false negative)
    pub const YELLOW: Color = Color::rgb(1.0, 1.0, 0.0); // False positive
    pub const CYAN: Color = Color::rgb(0.0, 1.0, 1.0); // Detected centroid
    pub const MAGENTA: Color = Color::rgb(1.0, 0.0, 1.0); // True centroid
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const ORANGE: Color = Color::rgb(1.0, 0.65, 0.0); // Saturated
}

/// Create a comparison image showing ground truth and detected stars.
///
/// # Arguments
/// * `pixels` - Background image pixels
/// * `width`, `height` - Image dimensions
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
pub fn create_comparison_image(
    pixels: &[f32],
    width: usize,
    height: usize,
    ground_truth: &[GroundTruthStar],
    detected: &[Star],
    match_radius: f32,
) -> Image {
    let mut image = gray_to_rgb_image_stretched(pixels, width, height);

    // Match detected stars to ground truth
    let matches = match_stars(ground_truth, detected, match_radius);

    // Draw ground truth stars
    for (i, truth) in ground_truth.iter().enumerate() {
        let cx = truth.x;
        let cy = truth.y;
        let radius = (truth.fwhm * 1.5).max(5.0);

        // Color depends on whether it was detected
        let color = if matches.matched_truth.contains(&i) {
            colors::GREEN // Detected
        } else {
            colors::RED // Missed
        };

        draw_circle(&mut image, cx, cy, radius, color, 1.0);

        // Draw true centroid position
        if !matches.matched_truth.contains(&i) {
            draw_cross(&mut image, cx, cy, 3.0, colors::MAGENTA, 1.0);
        }
    }

    // Draw detected stars
    for (i, det) in detected.iter().enumerate() {
        let cx = det.x;
        let cy = det.y;

        if matches.matched_detected.contains(&i) {
            // True positive - draw centroid cross
            draw_cross(&mut image, cx, cy, 3.0, colors::CYAN, 1.0);
        } else {
            // False positive - draw yellow circle
            let radius = (det.fwhm * 0.7).max(4.0);
            draw_circle(&mut image, cx, cy, radius, colors::YELLOW, 1.0);
            draw_cross(&mut image, cx, cy, 3.0, colors::YELLOW, 1.0);
        }
    }

    image
}

/// Create an image showing only ground truth positions.
pub fn create_ground_truth_image(
    pixels: &[f32],
    width: usize,
    height: usize,
    ground_truth: &[GroundTruthStar],
) -> Image {
    let mut image = gray_to_rgb_image_stretched(pixels, width, height);

    for truth in ground_truth {
        let cx = truth.x;
        let cy = truth.y;
        let radius = (truth.fwhm * 1.5).max(5.0);

        let color = if truth.is_saturated {
            colors::ORANGE
        } else {
            colors::BLUE
        };

        draw_circle(&mut image, cx, cy, radius, color, 1.0);
        draw_cross(&mut image, cx, cy, 3.0, color, 1.0);
    }

    image
}

/// Create an image showing only detected stars.
pub fn create_detection_image(
    pixels: &[f32],
    width: usize,
    height: usize,
    detected: &[Star],
) -> Image {
    let mut image = gray_to_rgb_image_stretched(pixels, width, height);

    for det in detected {
        let cx = det.x;
        let cy = det.y;
        let radius = (det.fwhm * 0.7).max(4.0);

        draw_circle(&mut image, cx, cy, radius, colors::GREEN, 1.0);
        draw_cross(&mut image, cx, cy, 3.0, colors::GREEN, 1.0);
    }

    image
}

/// Result of matching detected stars to ground truth.
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// Indices of matched ground truth stars
    pub matched_truth: Vec<usize>,
    /// Indices of matched detected stars
    pub matched_detected: Vec<usize>,
    /// Pairs of (truth_idx, detected_idx, distance)
    pub pairs: Vec<(usize, usize, f32)>,
}

/// Match detected stars to ground truth using nearest neighbor.
pub fn match_stars(
    ground_truth: &[GroundTruthStar],
    detected: &[Star],
    max_distance: f32,
) -> MatchResult {
    let mut matched_truth = Vec::new();
    let mut matched_detected = Vec::new();
    let mut pairs = Vec::new();

    let max_dist_sq = max_distance * max_distance;

    // For each ground truth star, find closest detected star
    for (ti, truth) in ground_truth.iter().enumerate() {
        let mut best_dist_sq = f32::MAX;
        let mut best_di = None;

        for (di, det) in detected.iter().enumerate() {
            // Skip already matched detected stars
            if matched_detected.contains(&di) {
                continue;
            }

            let dx = det.x - truth.x;
            let dy = det.y - truth.y;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq < best_dist_sq && dist_sq < max_dist_sq {
                best_dist_sq = dist_sq;
                best_di = Some(di);
            }
        }

        if let Some(di) = best_di {
            matched_truth.push(ti);
            matched_detected.push(di);
            pairs.push((ti, di, best_dist_sq.sqrt()));
        }
    }

    MatchResult {
        matched_truth,
        matched_detected,
        pairs,
    }
}

/// Draw centroid refinement path on an image.
pub fn draw_centroid_path(image: &mut Image, positions: &[(f32, f32)], color: Color) {
    for window in positions.windows(2) {
        let (x1, y1) = window[0];
        let (x2, y2) = window[1];

        // Draw line between consecutive positions
        draw_line(image, x1, y1, x2, y2, color, 1.0);
    }

    // Mark each position with a small dot
    for (i, &(x, y)) in positions.iter().enumerate() {
        let intensity = (i as f32 / positions.len() as f32) * 0.5 + 0.5;
        let scaled_color = Color::rgb(
            color.r * intensity,
            color.g * intensity,
            color.b * intensity,
        );

        imaginarium::drawing::draw_dot(image, x, y, 1.0, scaled_color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_stars_perfect() {
        let truth = vec![
            GroundTruthStar {
                x: 10.0,
                y: 10.0,
                flux: 1.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                is_saturated: false,
                angle: 0.0,
            },
            GroundTruthStar {
                x: 50.0,
                y: 50.0,
                flux: 1.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                is_saturated: false,
                angle: 0.0,
            },
        ];

        let detected = vec![
            Star {
                x: 10.1,
                y: 10.1,
                flux: 1.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 50.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            },
            Star {
                x: 50.2,
                y: 49.8,
                flux: 1.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 50.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            },
        ];

        let result = match_stars(&truth, &detected, 5.0);

        assert_eq!(result.matched_truth.len(), 2);
        assert_eq!(result.matched_detected.len(), 2);
    }

    #[test]
    fn test_match_stars_with_false_positive() {
        let truth = vec![GroundTruthStar {
            x: 10.0,
            y: 10.0,
            flux: 1.0,
            fwhm: 3.0,
            eccentricity: 0.0,
            is_saturated: false,
            angle: 0.0,
        }];

        let detected = vec![
            Star {
                x: 10.1,
                y: 10.1,
                flux: 1.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 50.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            },
            Star {
                x: 100.0,
                y: 100.0,
                flux: 1.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 50.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            }, // False positive
        ];

        let result = match_stars(&truth, &detected, 5.0);

        assert_eq!(result.matched_truth.len(), 1);
        assert_eq!(result.matched_detected.len(), 1);
        // Second detected star is not matched
        assert!(!result.matched_detected.contains(&1));
    }
}
