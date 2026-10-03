//! [`InverseWarp`]: a registration warp run backwards, from target pixels to reference pixels.

use glam::{DMat2, DVec2};

use crate::stacking::registration::distortion::sip::SipPolynomial;
use crate::stacking::registration::transform::Transform;

/// The step below which the Newton inversion of a SIP correction stops, in pixels.
///
/// Newton's error after a step of size `δ` is about `½·|c''|·δ²` in the correction's own scale; a
/// SIP field bends by well under 1e-2 per pixel, so stopping once a step is under 1e-6 px leaves an
/// error below 1e-14 px — under the f64 resolution of an image coordinate.
const NEWTON_TOLERANCE_PX: f64 = 1e-6;

/// Newton steps before a point is given up as not invertible.
///
/// From the start value `T⁻¹(t)` the error is the correction itself, tens of pixels at most, and
/// Newton halves its digits' distance to the root every step once the error is under the field's
/// curvature radius, which a fit's field is from the start: four or five steps converge. Thirty-two
/// leaves the margin for a strong field and still stops a point past a fold — where `r + c(r) = u`
/// has no solution — instead of letting it wander.
const NEWTON_MAX_ITERATIONS: usize = 32;

/// A [`WarpTransform`](crate::WarpTransform) run backwards: target pixels to reference pixels.
///
/// The forward warp is `t = T(r + c(r))`; the inverse is `T⁻¹` and then the solution of
/// `r + c(r) = T⁻¹(t)`, found by Newton iteration from `T⁻¹(t)`. Without a SIP correction it is
/// `T⁻¹` alone.
#[derive(Debug, Clone)]
pub struct InverseWarp {
    to_reference: Transform,
    sip: Option<SipPolynomial>,
}

/// One point taken back through an [`InverseWarp`]: where it lands, and the Jacobian of the inverse
/// map there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InverseMapped {
    pub position: DVec2,
    /// Columns are the images of the x and y steps of the target point.
    pub jacobian: DMat2,
}

impl InverseWarp {
    pub(crate) const fn new(to_reference: Transform, sip: Option<SipPolynomial>) -> Self {
        Self { to_reference, sip }
    }

    /// The reference point `t` came from, and the Jacobian of the inverse there; `None` when the
    /// correction cannot be inverted at `t` — the Newton iteration met a singular Jacobian, left the
    /// finite numbers, or did not converge in [`NEWTON_MAX_ITERATIONS`] steps. A position that did
    /// not converge is never returned.
    pub fn apply(&self, t: DVec2) -> Option<InverseMapped> {
        let u = self.to_reference.apply(t);
        let linear = self.to_reference.jacobian(t);
        let Some(sip) = &self.sip else {
            return Some(InverseMapped {
                position: u,
                jacobian: linear,
            });
        };
        if !u.is_finite() {
            return None;
        }
        let mut r = u;
        for _ in 0..NEWTON_MAX_ITERATIONS {
            let correction = sip.jacobian(r);
            let determinant = correction.determinant();
            if determinant == 0.0 || !determinant.is_finite() {
                return None;
            }
            let step = correction.inverse() * (sip.correct(r) - u);
            r -= step;
            if !r.is_finite() {
                return None;
            }
            if step.length() < NEWTON_TOLERANCE_PX {
                // `r ↦ r + c(r)` has Jacobian `I + ∂c`, so its inverse at `u` has the inverse of
                // that, and the chain rule puts `T⁻¹`'s own Jacobian after it.
                return Some(InverseMapped {
                    position: r,
                    jacobian: sip.jacobian(r).inverse() * linear,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec2;

    use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
    use crate::stacking::registration::transform::inverse_warp::NEWTON_TOLERANCE_PX;
    use crate::stacking::registration::transform::{Transform, WarpTransform};

    /// A SIP fit of `field` over a grid on `[-extent, extent]²` about the origin, on `transform`.
    fn sip_on(transform: Transform, extent: i32, field: impl Fn(DVec2) -> DVec2) -> WarpTransform {
        let mut reference = Vec::new();
        for y in (-extent..=extent).step_by(5) {
            for x in (-extent..=extent).step_by(5) {
                reference.push(DVec2::new(f64::from(x), f64::from(y)));
            }
        }
        let target: Vec<DVec2> = reference
            .iter()
            .map(|&r| transform.apply(r + field(r)))
            .collect();
        let config = SipConfig {
            order: 3,
            reference_point: Some(DVec2::ZERO),
            clip_iterations: 0,
            ..SipConfig::default()
        };
        let sip = SipPolynomial::fit_from_transform(&reference, &target, &transform, &config)
            .unwrap()
            .polynomial;
        WarpTransform::with_sip(transform, sip)
    }

    /// Back through the inverse lands where the forward warp started, to the Newton tolerance; the
    /// inverse's Jacobian is the inverse of the forward one; and without SIP it is `T⁻¹` exactly.
    #[test]
    fn the_inverse_undoes_the_forward_warp() {
        let transform = Transform::similarity(DVec2::new(12.0, -7.0), 0.3, 1.05);
        let warp = sip_on(transform, 200, |r| r * 1e-6 * r.length_squared());
        let inverse = warp.inverse();
        for r in [
            DVec2::ZERO,
            DVec2::new(150.0, -90.0),
            DVec2::new(-180.0, 175.0),
        ] {
            let t = warp.apply(r);
            let mapped = inverse.apply(t).unwrap();
            assert!(
                (mapped.position - r).length() < NEWTON_TOLERANCE_PX,
                "{r:?} came back as {:?}",
                mapped.position
            );
            // Forward Jacobian by the chain rule, `J_T · (I + ∂c)`; its product with the inverse's
            // is the identity to the rounding of two 2×2 products, 1e-12 at these entry sizes.
            let forward = transform.jacobian(warp.sip.as_ref().unwrap().correct(r))
                * warp.sip.as_ref().unwrap().jacobian(r);
            let identity = forward * mapped.jacobian;
            for (actual, expected) in identity
                .to_cols_array()
                .into_iter()
                .zip([1.0, 0.0, 0.0, 1.0])
            {
                assert!((actual - expected).abs() < 1e-12, "{identity:?}");
            }
        }

        let plain = WarpTransform::new(transform).inverse();
        let t = DVec2::new(33.0, 44.0);
        assert_eq!(
            plain.apply(t).unwrap().position,
            transform.inverse().apply(t)
        );
    }

    /// Past a fold the correction has no inverse, and the iteration says so rather than returning
    /// where it stopped. `c(x) = −0.01·x²` along x makes `x + c(x)` peak at 25 (at x = 50), so a
    /// target at x = 30 has no preimage; one at x = 10 has `x = (1 − √0.6) / 0.02` ≈ 11.27.
    #[test]
    fn a_point_past_a_fold_does_not_invert() {
        let warp = sip_on(Transform::identity(), 40, |r| {
            DVec2::new(-0.01 * r.x * r.x, 0.0)
        });
        let inverse = warp.inverse();
        assert!(inverse.apply(DVec2::new(30.0, 0.0)).is_none());
        let inside = inverse.apply(DVec2::new(10.0, 5.0)).unwrap().position;
        let expected = (1.0 - 0.6f64.sqrt()) / 0.02;
        assert!((inside.x - expected).abs() < 1e-6, "{inside:?}");
        assert!((inside.y - 5.0).abs() < 1e-6, "{inside:?}");
    }
}
