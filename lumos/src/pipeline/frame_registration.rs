//! [`FrameRegistration`]: how one light registered to the alignment reference.

use crate::registration::result::RegistrationError;
use crate::registration::transform::WarpTransform;

/// How one light registered to the alignment reference.
#[derive(Debug, Clone)]
pub enum FrameRegistration {
    /// The alignment reference itself, stacked unwarped.
    Reference,
    /// Registered and warped into the stack.
    Registered {
        /// The warp the light was resampled through, from reference to light coordinates; boxed,
        /// as its SIP correction would size every record.
        warp: Box<WarpTransform>,
        /// The star pairs the fit rests on.
        inliers: usize,
        /// Their RMS residual, in pixels.
        rms_error: f64,
    },
    /// Left out of the stack: why registration refused it.
    Dropped(RegistrationError),
}

impl FrameRegistration {
    /// Whether the light is in the stack: the reference, or registered.
    pub const fn is_stacked(&self) -> bool {
        !matches!(self, Self::Dropped(_))
    }
}
