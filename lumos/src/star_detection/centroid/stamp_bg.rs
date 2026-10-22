//! The sky a star is measured against, beyond the global map the residual already removed.
//!
//! Either nothing — the map's level stands — or a robust median of the residual over an annulus
//! around the star, the local sky the tiled map smoothed over, at the cost of needing enough
//! in-bounds samples to be trustworthy.

use std::ops::{Range, RangeInclusive};

use arrayvec::ArrayVec;
use glam::DVec2;

use crate::image::masked_plane::MaskedPlane;
use crate::math::size2us::Size2us;
use crate::math::statistics::clipped_stats::{ClippedStats, SigmaClip};
use crate::math::vec2us::Vec2us;
use crate::star_detection::centroid::measure_grid::AnnulusRadii;

/// Flat per-stamp sky: the residual's offset from zero and its noise, valid at the stamp scale.
#[derive(Debug, Clone, Copy)]
pub(super) struct StampBg {
    /// The sky the residual still carries here, subtracted from every stamp pixel.
    pub(super) offset: f32,
    pub(super) noise: f32,
    /// The samples the offset and the noise were measured from.
    pub(super) samples: usize,
}

impl StampBg {
    /// The residual's sigma-clipped median and σ over `annulus` about `pos`, less the pixels its
    /// mask names, whose inner radius keeps the star's flux out; `None` when fewer than 10 of its
    /// pixels lie in the frame.
    pub(super) fn measure(
        residual: MaskedPlane<'_>,
        pos: DVec2,
        annulus: AnnulusRadii,
    ) -> Option<Self> {
        let MaskedPlane {
            pixels: residual,
            mask: no_data,
        } = residual;
        let spans = || AnnulusSpans::about(pos, annulus, Size2us::from(residual));
        let measured = |&(x, y): &(usize, usize)| {
            no_data.is_none_or(|no_data| !no_data.get_at(Vec2us::new(x, y)))
        };
        // The annulus pixels inside the frame that hold a measurement, row by row, as image
        // positions.
        let pixels = || {
            spans()
                .flat_map(|(y, xs)| xs.map(move |x| (x, y)))
                .filter(measured)
        };
        let count = match no_data {
            None => spans().map(|(_, xs)| xs.len()).sum(),
            Some(_) => pixels().count(),
        };
        if count < 10 {
            return None;
        }
        let stride = count.div_ceil(MAX_ANNULUS_SAMPLES);

        // Stack scratch: this runs per star inside the parallel measure loop, so it must not allocate.
        let mut values: ArrayVec<f32, MAX_ANNULUS_SAMPLES> = ArrayVec::new();
        values.extend(pixels().step_by(stride).map(|(x, y)| residual.row(y)[x]));
        let mut deviations: ArrayVec<f32, MAX_ANNULUS_SAMPLES> = ArrayVec::new();
        let samples = values.len();
        let stats = ClippedStats::sigma_clipped(
            &mut values,
            &mut deviations,
            SigmaClip {
                kappa: 3.0,
                iterations: 2,
            },
        );
        Some(Self {
            offset: stats.median,
            noise: stats.sigma,
            samples,
        })
    }
}

/// The most annulus pixels the sky is measured from: the clipped median of 2048 samples errs by
/// `1.25·σ/√2048`, under 3% of σ, and a wider annulus is subsampled evenly down to it.
const MAX_ANNULUS_SAMPLES: usize = 2048;

/// The pixels of an annulus inside a frame, as each row's spans of x: the pixels at `dx, dy` from
/// the rounded centre with `inner² ≤ dx² + dy² ≤ outer²` lie, on row `dy`, within
/// `|dx| ≤ ⌊√(outer² − dy²)⌋`, and outside `|dx| ≤ ⌊√(inner² − dy² − 1)⌋` where `dy² < inner²`.
#[derive(Debug)]
struct AnnulusSpans {
    centre_x: isize,
    centre_y: isize,
    inner_sq: usize,
    outer_sq: usize,
    size: Size2us,
    rows: RangeInclusive<isize>,
    /// The right span of the row the left one was handed out of.
    pending: Option<(usize, Range<usize>)>,
}

impl AnnulusSpans {
    fn about(pos: DVec2, annulus: AnnulusRadii, size: Size2us) -> Self {
        let reach =
            isize::try_from(annulus.outer).expect("an annulus is a few hundred pixels across");
        Self {
            centre_x: pos.x.round() as isize,
            centre_y: pos.y.round() as isize,
            inner_sq: annulus.inner * annulus.inner,
            outer_sq: annulus.outer * annulus.outer,
            size,
            rows: -reach..=reach,
            pending: None,
        }
    }

    /// `centre_x + dx` for `dx` in `span`, held to the frame's columns.
    fn clipped(&self, span: Range<isize>) -> Range<usize> {
        let width = isize::try_from(self.size.width).expect("a frame's width fits an isize");
        let col = |dx: isize| {
            usize::try_from((self.centre_x + dx).clamp(0, width)).expect("clamped non-negative")
        };
        col(span.start)..col(span.end)
    }
}

impl Iterator for AnnulusSpans {
    /// A row's image y, and a span of its x.
    type Item = (usize, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(right) = self.pending.take() {
            return Some(right);
        }
        loop {
            let dy = self.rows.next()?;
            let Some(y) = usize::try_from(self.centre_y + dy)
                .ok()
                .filter(|&y| y < self.size.height)
            else {
                continue;
            };
            let dy_sq = dy.unsigned_abs().pow(2);
            let outer = isize::try_from((self.outer_sq - dy_sq).isqrt()).expect("within reach");
            let Some(hole) = self.inner_sq.checked_sub(dy_sq + 1) else {
                return Some((y, self.clipped(-outer..outer + 1)));
            };
            let hole = isize::try_from(hole.isqrt()).expect("within reach");
            self.pending = Some((y, self.clipped(hole + 1..outer + 1)));
            return Some((y, self.clipped(-outer..-hole)));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::star_detection::centroid::stamp_bg::*;

    /// The spans hold exactly the pixels a walk over the annulus' square keeps, in its row-major
    /// order: about the frame's middle, past each edge and corner, and with an annulus wider than
    /// the frame.
    #[test]
    fn the_spans_hold_the_annulus_pixels_in_order() {
        let size = Size2us::new(40, 30);
        let annuli = [
            AnnulusRadii { inner: 5, outer: 9 },
            AnnulusRadii {
                inner: 17,
                outer: 24,
            },
            AnnulusRadii { inner: 1, outer: 1 },
        ];
        let poss = [
            DVec2::new(20.0, 15.0),
            DVec2::new(0.0, 0.0),
            DVec2::new(39.4, 29.0),
            DVec2::new(3.6, 27.2),
            DVec2::new(-2.0, 15.0),
        ];
        for annulus in annuli {
            for pos in poss {
                let (cx, cy) = (pos.x.round() as isize, pos.y.round() as isize);
                let reach = annulus.outer as isize;
                let (inner_sq, outer_sq) = (annulus.inner.pow(2), annulus.outer.pow(2));
                let mut walked = Vec::new();
                for dy in -reach..=reach {
                    for dx in -reach..=reach {
                        let (x, y) = (cx + dx, cy + dy);
                        let r_sq = dx.unsigned_abs().pow(2) + dy.unsigned_abs().pow(2);
                        let inside = usize::try_from(x)
                            .ok()
                            .filter(|&x| x < 40)
                            .zip(usize::try_from(y).ok().filter(|&y| y < 30));
                        if let Some(at) = inside.filter(|_| (inner_sq..=outer_sq).contains(&r_sq)) {
                            walked.push(at);
                        }
                    }
                }
                let spanned: Vec<_> = AnnulusSpans::about(pos, annulus, size)
                    .flat_map(|(y, xs)| xs.map(move |x| (x, y)))
                    .collect();
                assert_eq!(spanned, walked, "{annulus:?} about {pos}");
            }
        }
    }
}
