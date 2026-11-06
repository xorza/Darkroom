//! What an input stage leaves the frame.

use palantir::InputOutcome;

use crate::gui::frame::relayout::Relayout;

/// What an input stage leaves the frame: what its records may read of the input, and whether a
/// pass it ran stranded the canvas's geometry. Each level adds its own to it on the way up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputEnd {
    pub(crate) outcome: InputOutcome,
    pub(crate) relayout: Relayout,
}
