//! [`RowPositions`]: one output row's source positions, evaluated once for every reader.

use glam::DVec2;

use crate::math::dmat3::DMat3;
use crate::math::size2us::Size2us;
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::transform::TransformType;
use crate::registration::warp_transform::WarpTransform;

/// The source position of every pixel of one output row, evaluated once and read by every channel,
/// the quality maps and the validity warp.
///
/// A row of a model without SIP is affine in `x` in its numerators and, for a homography, its
/// denominator, so the row's `y` terms are taken once and each pixel adds its `x` terms. The
/// arithmetic is [`WarpTransform::apply`]'s, operation for operation, so the positions are the
/// same bits. A SIP correction is not affine in `x`, but along one row it is a polynomial in `x`
/// alone ([`SipPolynomial::row`](crate::registration::distortion::sip::SipPolynomial::row)).
#[derive(Debug, Default)]
pub(super) struct RowPositions {
    positions: Vec<Option<SourcePosition>>,
}

impl RowPositions {
    /// Fill with the positions output row `y` samples in a `source`-sized image, the output frame
    /// the source's size.
    pub(super) fn fill(&mut self, y: usize, transform: &WarpTransform, source: Size2us) {
        let width = source.width;
        self.positions.clear();
        self.positions.reserve_exact(width);
        let y = y as f64;
        if let Some(sip) = &transform.sip {
            let row = sip.row(y);
            self.positions.extend((0..width).map(|x| {
                SourcePosition::within(transform.transform.apply(row.correct(x as f64)), source)
            }));
            return;
        }
        let m = transform.transform.matrix();
        let (by, ey) = (m[1] * y, m[4] * y);
        if transform.transform.transform_type() == TransformType::Homography {
            let hy = m[7] * y;
            self.positions.extend((0..width).map(|x| {
                let x = x as f64;
                let w = m[6] * x + hy + m[8];
                // `DMat3::transform_point`'s horizon: a point on or past it maps outside.
                if !DMat3::maps_forward(w) {
                    return None;
                }
                let p = DVec2::new((m[0] * x + by + m[2]) / w, (m[3] * x + ey + m[5]) / w);
                SourcePosition::within(p, source)
            }));
        } else {
            // `w` is 1 exactly, and dividing by 1 is the identity, so it is left out.
            self.positions.extend((0..width).map(|x| {
                let x = x as f64;
                SourcePosition::within(
                    DVec2::new(m[0] * x + by + m[2], m[3] * x + ey + m[5]),
                    source,
                )
            }));
        }
    }

    pub(super) fn positions(&self) -> &[Option<SourcePosition>] {
        &self.positions
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
