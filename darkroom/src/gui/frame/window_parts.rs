//! [`WindowParts`]: what one record of the window takes past its context.

use crate::core::io::preferences::Preferences;
use crate::core::status_log::StatusMessage;
use crate::gui::frame::requests::Requests;

/// What a window's record takes past its context: the preferences its forms edit in place, the
/// requests its surfaces raise, and what only its status bar reads. The sinks outlive the frame,
/// and the record borrows them for its pass.
#[derive(Debug)]
pub(crate) struct WindowParts<'a> {
    pub(crate) preferences: &'a mut Preferences,
    pub(crate) requests: &'a mut Requests,
    pub(crate) status: StatusInputs<'a>,
}

/// The status bar's inputs that `App` owns beside the run projection.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct StatusInputs<'a> {
    /// The last failure or warning (the engine's
    /// [`StatusLog::cur`](crate::core::status_log::StatusLog::cur)), shown until a success of its
    /// family clears it.
    pub(crate) message: Option<StatusMessage<'a>>,
    /// This process's resident bytes (see
    /// [`ProcessMemory`](crate::gui::state::process_memory::ProcessMemory)), rendered as the bar's
    /// `MEM` clause.
    pub(crate) process_memory: u64,
}
