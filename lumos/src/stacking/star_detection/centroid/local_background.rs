//! The sky level a star is measured against.
//!
//! Either the global background map's value at the star, or a robust median over an annulus
//! around it — the annulus tracks structure the tiled map smooths over, at the cost of needing
//! enough in-bounds samples to be trustworthy.

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::statistics::ClippedStats;
use crate::stacking::star_detection::centroid::MAX_ANNULUS_PIXELS;

/// Flat per-stamp sky estimate: one (background, noise) pair valid at the stamp
/// scale, as opposed to the per-pixel tiled global map.
#[derive(Debug, Clone, Copy)]
pub(super) struct LocalBackground {
    pub(super) bg: f32,
    pub(super) noise: f32,
}

/// Compute local background and noise using an annular region around the star.
///
/// The inner radius excludes the star's flux, and the outer radius samples
/// the local sky. Uses sigma-clipped median for robustness.
///
/// # Arguments
/// * `pixels` - Image data
/// * `width` - Image width
/// * `height` - Image height
/// * `pos` - Star center position
/// * `inner_radius` - Inner radius of annulus (excludes star)
/// * `outer_radius` - Outer radius of annulus
///
/// # Returns
/// The local background/noise, or None if not enough valid pixels
pub(super) fn compute_annulus_background(
    pixels: &Buffer2<f32>,
    pos: DVec2,
    inner_radius: usize,
    outer_radius: usize,
) -> Option<LocalBackground> {
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let inner_r2 = (inner_radius * inner_radius) as f32;
    let outer_r2 = (outer_radius * outer_radius) as f32;

    // Use stack-allocated ArrayVec to avoid heap allocation
    let mut values: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();

    let width = pixels.width() as isize;
    let height = pixels.height() as isize;
    let outer_r_i32 = outer_radius as i32;
    for dy in -outer_r_i32..=outer_r_i32 {
        // Row bound first so the row slice — and its bounds check — is taken once, not per
        // column. The annulus can hang off the frame, so the row may not exist at all.
        let y = icy + dy as isize;
        if y < 0 || y >= height {
            continue;
        }
        let row = pixels.row(y as usize);

        for dx in -outer_r_i32..=outer_r_i32 {
            let r2 = (dx * dx + dy * dy) as f32;
            if r2 < inner_r2 || r2 > outer_r2 {
                continue;
            }

            let x = icx + dx as isize;
            if x >= 0 && x < width {
                values.push(row[x as usize]);
            }
        }
    }

    if values.len() < 10 {
        return None;
    }

    // Sigma-clipped median (2 iterations, 3-sigma clip)
    let stats = sigma_clipped_median_mad(&mut values, 3.0, 2);
    Some(LocalBackground {
        bg: stats.median,
        noise: stats.sigma,
    })
}

/// Compute sigma-clipped median and MAD using the shared implementation.
/// Uses stack-allocated ArrayVec for deviations to avoid heap allocation.
#[inline]
fn sigma_clipped_median_mad(values: &mut [f32], kappa: f32, iterations: usize) -> ClippedStats {
    // Stack scratch: this runs per star inside the parallel measure loop, so it must not allocate.
    let mut deviations: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();
    ClippedStats::sigma_clipped(values, &mut deviations, kappa, iterations)
}
