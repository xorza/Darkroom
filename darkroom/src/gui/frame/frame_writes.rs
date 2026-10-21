//! [`FrameWrites`]: what one frame's record may change.

use crate::core::io::preferences::Preferences;
use crate::gui::frame::requests::Requests;

/// What a frame may write: the preferences its forms edit in place, and the
/// requests its surfaces raise. Both outlive the frame; the record borrows them
/// for its pass.
#[derive(Debug)]
pub(crate) struct FrameWrites<'a> {
    pub(crate) preferences: &'a mut Preferences,
    pub(crate) requests: &'a mut Requests,
}
