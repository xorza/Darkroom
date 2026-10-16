//! [`FramePlane`]: which of a frame's planes a check is about, and the range its values take.

use std::fmt;
use std::fmt::{Display, Formatter};

/// Which of a frame's planes a validation failure is about.
///
/// Names the plane in the errors below, and picks the range each one must satisfy: coverage is a
/// fraction of a pixel that had support, confidence an interpolation's noise factor with no upper
/// bound. Carrying the kind rather than its label is what keeps that rule out of a string
/// comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePlane {
    /// One of the image's color planes.
    Channel,
    /// Per-pixel warp support, in `[0, 1]`.
    Coverage,
    /// Per-pixel interpolation confidence, non-negative.
    Confidence,
    /// A drizzled frame's per-pixel drop weight `Σw`, non-negative.
    DropWeight,
    /// The per-pixel weight a caller gives a drizzle frame's input pixels, non-negative.
    PixelWeight,
}

impl FramePlane {
    /// Whether `value` is in range for this plane. Non-finite is out of range for all of them.
    pub(crate) fn accepts(self, value: f32) -> bool {
        value.is_finite()
            && match self {
                // Finiteness is the whole rule for image data: dark subtraction takes a calibrated
                // channel below zero legitimately.
                Self::Channel => true,
                Self::Coverage => (0.0..=1.0).contains(&value),
                Self::Confidence | Self::DropWeight | Self::PixelWeight => value >= 0.0,
            }
    }
}

impl Display for FramePlane {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Channel => "a channel",
            Self::Coverage => "coverage",
            Self::Confidence => "confidence",
            Self::DropWeight => "drop weight",
            Self::PixelWeight => "pixel weight map",
        })
    }
}
