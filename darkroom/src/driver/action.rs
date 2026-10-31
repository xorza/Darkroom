//! [`Action`]: one step the driver plays into the window: palantir's steps over the editor's
//! places, and the answer to a file dialog.

use crate::driver::locator::EditorTarget;
use crate::gui::dialogs::DialogAnswer;

/// One step of input, over the editor's places as well as palantir's.
pub type Play = palantir::Action<EditorTarget>;

/// Pointer moves a drag takes from its start to its end when the call names none.
pub const DRAG_STEPS: u32 = palantir::Action::DRAG_STEPS;

/// One step the driver plays. It runs frames until the editor settles after each.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A step a person takes at the window, played as palantir plays it.
    Play(Play),
    /// Queue the answer the next file dialog gets.
    Answer(DialogAnswer),
}
