//! [`RowPositions`]: one output row's source positions, evaluated once for every reader.

use glam::DVec2;

use crate::math::size2us::Size2us;
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::transform::{TransformType, WarpTransform};

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
    /// Fill with the positions output row `y` of `width` pixels samples in a `source`-sized image.
    pub(super) fn fill(
        &mut self,
        y: usize,
        width: usize,
        transform: &WarpTransform,
        source: Size2us,
    ) {
        self.positions.clear();
        self.positions.reserve(width);
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
                // `DMat3::transform_point`'s horizon: a point there maps to infinity, outside.
                if w.abs() <= f64::EPSILON {
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

#[cfg(test)]
mod tests {
    use glam::DVec2;

    use crate::math::size2us::Size2us;
    use crate::registration::distortion::sip::{SipConfig, SipPolynomial};
    use crate::registration::resample::row_positions::RowPositions;
    use crate::registration::resample::source_position::SourcePosition;
    use crate::registration::transform::{Transform, WarpTransform};

    /// Every row of an affine model or a homography is the bits `WarpTransform::apply` gives, split
    /// by `SourcePosition::within`, and positions past the footprint are `None`. A SIP row sums its
    /// polynomial in another order, so it agrees to rounding instead: the correction here reaches
    /// `1e-6·180³` ≈ 6 px, so the reordering moves it by a few 1e-15 px, and the f32 fraction of
    /// the split, under 1, rounds to half its 6e-8 spacing — 1e-7 holds both.
    #[test]
    fn a_row_is_what_apply_gives_pixel_by_pixel() {
        let source = Size2us::new(300, 200);
        let center = DVec2::new(150.0, 100.0);
        let affine = Transform::similarity(DVec2::new(-12.5, 7.25), 0.03, 1.01);
        let mut reference = Vec::new();
        for y in (0..200).step_by(20) {
            for x in (0..300).step_by(20) {
                reference.push(DVec2::new(f64::from(x), f64::from(y)));
            }
        }
        let target: Vec<DVec2> = reference
            .iter()
            .map(|&r| {
                let d = r - center;
                affine.apply(r + d * 1e-6 * d.length_squared())
            })
            .collect();
        let config = SipConfig {
            order: 3,
            reference_point: Some(center),
        };
        let sip = SipPolynomial::fitted_under(
            &affine,
            &reference,
            &target,
            config.order,
            config.reference_point.unwrap(),
        );
        let transforms = [
            WarpTransform::new(affine),
            WarpTransform::new(Transform::homography([
                1.02, 0.01, 30.0, -0.015, 0.98, -4.0, 4e-5, -3e-5,
            ])),
            WarpTransform::with_sip(affine, sip),
        ];
        let mut row = RowPositions::default();
        for transform in &transforms {
            let mut outside = 0;
            for y in [0, 57, 199] {
                row.fill(y, 320, transform, source);
                assert_eq!(row.positions().len(), 320);
                for (x, &position) in row.positions().iter().enumerate() {
                    let exact = transform.apply(DVec2::new(x as f64, y as f64));
                    let expected = SourcePosition::within(exact, source);
                    if transform.has_sip() {
                        assert_eq!(position.is_some(), expected.is_some(), "x = {x}, y = {y}");
                        if let Some(p) = position {
                            let split = DVec2::new(
                                f64::from(p.cell_x) + f64::from(p.fx),
                                f64::from(p.cell_y) + f64::from(p.fy),
                            );
                            assert!(
                                (split - exact).length() < 1e-7,
                                "x = {x}, y = {y}: {split:?} against {exact:?}"
                            );
                        }
                    } else {
                        assert_eq!(position, expected, "x = {x}, y = {y}");
                    }
                    outside += usize::from(position.is_none());
                }
            }
            assert!(outside > 0, "the rows reach past the footprint");
        }
    }
}
