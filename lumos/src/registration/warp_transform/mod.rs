//! [`WarpTransform`]: a transform and its distortion, as a warp applies them, and the inverse it
//! hands out.

pub(crate) mod inverse_warp;

use glam::{DMat2, DVec2};

use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::inverse_warp::InverseWarp;

/// A linear transform under an optional SIP distortion correction, as a warp applies them: from
/// each output pixel `p`, the source coordinate `linear.apply(sip.correct(p))`, or
/// `linear.apply(p)` with no correction. One value, so a warp cannot leave the correction out.
#[derive(Debug, Clone)]
pub struct WarpTransform {
    linear: Transform,
    sip: Option<SipPolynomial>,
}

impl From<Transform> for WarpTransform {
    fn from(linear: Transform) -> Self {
        Self { linear, sip: None }
    }
}

impl WarpTransform {
    /// `linear` under the correction `sip`.
    pub(crate) const fn with_sip(linear: Transform, sip: SipPolynomial) -> Self {
        Self {
            linear,
            sip: Some(sip),
        }
    }

    /// The linear transform from reference pixels to target pixels, after any correction.
    pub const fn linear(&self) -> Transform {
        self.linear
    }

    /// The SIP distortion correction, when the registration fitted one.
    pub const fn sip(&self) -> Option<&SipPolynomial> {
        self.sip.as_ref()
    }

    /// Compute the source coordinate for a given output pixel position.
    pub fn apply(&self, p: DVec2) -> DVec2 {
        let corrected = match &self.sip {
            Some(sip) => sip.correct(p),
            None => p,
        };
        self.linear.apply(corrected)
    }

    /// This warp run backwards, from target pixels to reference pixels — see [`InverseWarp`].
    ///
    /// # Panics
    /// When the transform has no usable inverse ([`Transform::try_inverse`]).
    pub fn inverse(&self) -> InverseWarp {
        InverseWarp::new(self.linear.inverse(), self.sip.clone())
    }

    /// The Jacobian of [`Self::apply`] at `p`: the transform's at the corrected point, times the
    /// correction's.
    pub(crate) fn jacobian(&self, p: DVec2) -> DMat2 {
        match &self.sip {
            Some(sip) => self.linear.jacobian(sip.correct(p)) * sip.jacobian(p),
            None => self.linear.jacobian(p),
        }
    }
}

#[cfg(test)]
mod tests;
