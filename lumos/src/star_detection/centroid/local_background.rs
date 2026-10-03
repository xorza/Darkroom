//! The sky a star is measured against, beyond the global map the residual already removed.
//!
//! Either nothing — the map's level stands — or a robust median of the residual over an annulus
//! around the star, the local sky the tiled map smoothed over, at the cost of needing enough
//! in-bounds samples to be trustworthy.

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::statistics::ClippedStats;
use crate::star_detection::centroid::MAX_ANNULUS_PIXELS;

/// Flat per-stamp sky: the residual's offset from zero and its noise, valid at the stamp scale.
#[derive(Debug, Clone, Copy)]
pub(super) struct LocalBackground {
    /// The sky the residual still carries here, subtracted from every stamp pixel.
    pub(super) offset: f32,
    pub(super) noise: f32,
}

/// The residual's sigma-clipped median and σ over the annulus between `inner_radius`, which keeps
/// the star's flux out, and `outer_radius` about `pos`; `None` when fewer than 10 of its pixels
/// lie in the frame.
pub(super) fn compute_annulus_background(
    residual: &Buffer2<f32>,
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

    let (width, height) = (residual.width(), residual.height());
    let reach = isize::try_from(outer_radius).expect("an annulus is a few dozen pixels across");
    for dy in -reach..=reach {
        // Row bound first so the row slice — and its bounds check — is taken once, not per
        // column. The annulus can hang off the frame, so the row may not exist at all.
        let Ok(y) = usize::try_from(icy + dy) else {
            continue;
        };
        if y >= height {
            continue;
        }
        let row = residual.row(y);

        for dx in -reach..=reach {
            let r2 = (dx * dx + dy * dy) as f32;
            if r2 < inner_r2 || r2 > outer_r2 {
                continue;
            }

            if let Ok(x) = usize::try_from(icx + dx)
                && x < width
            {
                values.push(row[x]);
            }
        }
    }

    if values.len() < 10 {
        return None;
    }

    // Stack scratch: this runs per star inside the parallel measure loop, so it must not allocate.
    let mut deviations: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();
    let stats = ClippedStats::sigma_clipped(&mut values, &mut deviations, 3.0, 2);
    Some(LocalBackground {
        offset: stats.median,
        noise: stats.sigma,
    })
}
