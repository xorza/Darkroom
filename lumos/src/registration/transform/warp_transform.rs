//! [`WarpTransform`]: a transform and its distortion, as a warp applies them.

use glam::{DMat2, DVec2};

use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::transform::inverse_warp::InverseWarp;

/// The unit roundoff of f64: half the gap between 1 and the next float.
use crate::image::linear_image::LinearImage;
use crate::registration::resample;
use crate::registration::resample::WarpResult;
use crate::registration::resample::warp_params::WarpParams;
use crate::registration::transform::Transform;

/// Combined transform + optional SIP distortion correction for warping.
///
/// Bundles a linear `Transform` with an optional `SipPolynomial` so that
/// callers of [`Self::warp`] cannot forget to include the SIP correction.
/// For each output pixel `p`, the source coordinate is:
/// `src = transform.apply(sip.correct(p))` when SIP is present,
/// or `src = transform.apply(p)` otherwise.
#[derive(Debug, Clone)]
pub struct WarpTransform {
    pub transform: Transform,
    pub sip: Option<SipPolynomial>,
}

impl WarpTransform {
    /// Warp an image to align with the reference frame.
    ///
    /// The `WarpTransform` bundles the linear transform with optional SIP distortion correction.
    /// Use `result.warp_transform()` to obtain one from a `RegistrationResult`, or
    /// `WarpTransform::new(transform)` for a plain transform.
    ///
    /// The output has the same dimensions and metadata as `image`; every output pixel is produced
    /// by inverse-mapping, so no input pixels are carried over. Returns a [`WarpResult`] carrying
    /// the aligned image and its quality maps (see that type).
    ///
    /// The maps are always produced: the pipeline's only caller needs both, so there is no
    /// unconditional cost to skip. They are geometry, not pixels, so each row's maps are written
    /// once, from the same source positions every channel of that row samples at.
    ///
    /// # Arguments
    /// * `image` - The source (target) image to warp
    /// * `params` - The interpolation and its clamp
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lumos::{LinearImage, RegistrationConfig, Star};
    ///
    /// # fn example(ref_stars: &[Star], target_stars: &[Star], target_image: &LinearImage)
    /// # -> Result<(), lumos::RegistrationError> {
    /// let config = RegistrationConfig::default();
    /// let result = config.register(ref_stars, target_stars)?;
    /// let aligned = result.warp_transform().warp(target_image, config.warp).image;
    /// # Ok(())
    /// # }
    /// ```
    pub fn warp(&self, image: &LinearImage, params: WarpParams) -> WarpResult {
        resample::warp(image, self, params)
    }

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
