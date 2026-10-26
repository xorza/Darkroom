//! [`InverseWarp`]: a registration warp run backwards, from target pixels to reference pixels.

use glam::DVec2;

use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::transform::Transform;

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

impl InverseWarp {
    pub(crate) const fn new(to_reference: Transform, sip: Option<SipPolynomial>) -> Self {
        Self { to_reference, sip }
    }

    /// The reference point `t` came from; `None` when the correction cannot be inverted at `t` —
    /// the Newton iteration met a singular Jacobian, left the finite numbers, or did not converge
    /// within its iteration cap. A position that did not converge is never returned.
    pub fn pos(&self, t: DVec2) -> Option<DVec2> {
        let u = self.to_reference.apply(t);
        let Some(sip) = &self.sip else {
            return Some(u);
        };
        if !u.is_finite() {
            return None;
        }
        let mut r = u;
        for _ in 0..NEWTON_MAX_ITERATIONS {
            let local = sip.local(r);
            let determinant = local.jacobian.determinant();
            if determinant == 0.0 || !determinant.is_finite() {
                return None;
            }
            let step = local.jacobian.inverse() * (local.corrected - u);
            r -= step;
            if !r.is_finite() {
                return None;
            }
            if step.length() < NEWTON_TOLERANCE_PX {
                return Some(r);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
