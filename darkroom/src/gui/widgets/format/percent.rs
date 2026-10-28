//! [`Percent`]: a zoom factor read as a percentage, formatted on demand.

use std::fmt;
use std::fmt::{Display, Formatter};

/// A zoom factor written as a percentage with no space before the sign, as a zoom control
/// labels it ("Zoom to 100%"): whole from 10 %, else two significant digits, so a fit far below
/// a view's range never reads as `0%`. The status bar's canvas zoom and the viewer's header read
/// it, so one zoom never renders two ways.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Percent(f32);

impl Percent {
    /// `factor` is the zoom, 1 at 100 %.
    pub(crate) const fn new(factor: f32) -> Self {
        Self(factor)
    }
}

impl Display for Percent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let percent = self.0 * 100.0;
        write!(f, "{percent:.0$}%", decimals(percent))
    }
}

/// The decimals `percent` shows: none from 10, else two significant digits.
fn decimals(percent: f32) -> usize {
    debug_assert!(percent > 0.0, "a zoom is positive");
    if percent >= 10.0 {
        return 0;
    }
    // The decimal exponent of the first significant digit: -1 for 0.4, -2 for 0.04.
    let magnitude = percent.log10().floor() as i32;
    // At most six, as an f32 holds about seven significant digits.
    usize::try_from((1 - magnitude).clamp(1, 6)).expect("clamped to 1..=6")
}
