//! [`WarpTransform`]: a transform and its distortion, as a warp applies them, and the inverse it
//! hands out.

pub(crate) mod inverse_warp;

use glam::{DMat2, DVec2};

use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::inverse_warp::InverseWarp;

/// Combined transform + optional SIP distortion correction for warping.
///
/// Bundles a linear `Transform` with an optional `SipPolynomial` so that a warp cannot leave out
/// the SIP correction.
/// For each output pixel `p`, the source coordinate is:
/// `src = transform.apply(sip.correct(p))` when SIP is present,
/// or `src = transform.apply(p)` otherwise.
#[derive(Debug, Clone)]
pub struct WarpTransform {
    pub transform: Transform,
    pub sip: Option<SipPolynomial>,
}

impl WarpTransform {
    /// Create a warp transform with no SIP correction.
    pub const fn new(transform: Transform) -> Self {
        Self {
            transform,
            sip: None,
        }
    }

    /// Create a warp transform with SIP distortion correction.
    pub const fn with_sip(transform: Transform, sip: SipPolynomial) -> Self {
        Self {
            transform,
            sip: Some(sip),
        }
    }

    /// Compute the source coordinate for a given output pixel position.
    pub fn apply(&self, p: DVec2) -> DVec2 {
        let corrected = match &self.sip {
            Some(sip) => sip.correct(p),
            None => p,
        };
        self.transform.apply(corrected)
    }

    /// This warp run backwards, from target pixels to reference pixels — see [`InverseWarp`].
    ///
    /// # Panics
    /// When the transform has no usable inverse ([`Transform::try_inverse`]).
    pub fn inverse(&self) -> InverseWarp {
        InverseWarp::new(self.transform.inverse(), self.sip.clone())
    }

    /// The Jacobian of [`Self::apply`] at `p`: the transform's at the corrected point, times the
    /// correction's.
    pub(crate) fn jacobian(&self, p: DVec2) -> DMat2 {
        match &self.sip {
            Some(sip) => self.transform.jacobian(sip.correct(p)) * sip.jacobian(p),
            None => self.transform.jacobian(p),
        }
    }

    /// Whether this transform has a nonlinear SIP component.
    pub const fn has_sip(&self) -> bool {
        self.sip.is_some()
    }
}

#[cfg(test)]
mod tests;
