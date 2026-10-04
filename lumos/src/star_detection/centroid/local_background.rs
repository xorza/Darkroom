//! The sky a star is measured against, beyond the global map the residual already removed.
//!
//! Either nothing — the map's level stands — or a robust median of the residual over an annulus
//! around the star, the local sky the tiled map smoothed over, at the cost of needing enough
//! in-bounds samples to be trustworthy.

use arrayvec::ArrayVec;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::statistics::ClippedStats;
use crate::star_detection::centroid::measure_grid::AnnulusRadii;

/// Flat per-stamp sky: the residual's offset from zero and its noise, valid at the stamp scale.
#[derive(Debug, Clone, Copy)]
pub(super) struct LocalBackground {
    /// The sky the residual still carries here, subtracted from every stamp pixel.
    pub(super) offset: f32,
    pub(super) noise: f32,
    /// The samples the offset and the noise were measured from.
    pub(super) samples: usize,
}

impl LocalBackground {
    /// The residual's sigma-clipped median and σ over `annulus` about `pos`, whose inner radius keeps
    /// the star's flux out; `None` when fewer than 10 of its pixels lie in the frame.
    pub(super) fn measure(
        residual: &Buffer2<f32>,
        pos: DVec2,
        annulus: AnnulusRadii,
    ) -> Option<Self> {
        let icx = pos.x.round() as isize;
        let icy = pos.y.round() as isize;
        let inner_r2 = annulus.inner * annulus.inner;
        let outer_r2 = annulus.outer * annulus.outer;
        let (width, height) = (residual.width(), residual.height());
        let reach =
            isize::try_from(annulus.outer).expect("an annulus is a few hundred pixels across");
        // The annulus pixels inside the frame, row by row, as image positions.
        let pixels = || {
            (-reach..=reach).flat_map(move |dy| {
                let y = usize::try_from(icy + dy).ok().filter(|&y| y < height);
                (-reach..=reach).filter_map(move |dx| {
                    let r2 = dx.unsigned_abs().pow(2) + dy.unsigned_abs().pow(2);
                    let x = usize::try_from(icx + dx).ok().filter(|&x| x < width)?;
                    (inner_r2..=outer_r2).contains(&r2).then_some((x, y?))
                })
            })
        };
        let count = pixels().count();
        if count < 10 {
            return None;
        }
        let stride = count.div_ceil(MAX_ANNULUS_SAMPLES);

        // Stack scratch: this runs per star inside the parallel measure loop, so it must not allocate.
        let mut values: ArrayVec<f32, MAX_ANNULUS_SAMPLES> = ArrayVec::new();
        values.extend(pixels().step_by(stride).map(|(x, y)| residual.row(y)[x]));
        let mut deviations: ArrayVec<f32, MAX_ANNULUS_SAMPLES> = ArrayVec::new();
        let samples = values.len();
        let stats = ClippedStats::sigma_clipped(&mut values, &mut deviations, 3.0, 2);
        Some(LocalBackground {
            offset: stats.median,
            noise: stats.sigma,
            samples,
        })
    }
}

/// The most annulus pixels the sky is measured from: the clipped median of 2048 samples errs by
/// `1.25·σ/√2048`, under 3% of σ, and a wider annulus is subsampled evenly down to it.
const MAX_ANNULUS_SAMPLES: usize = 2048;
