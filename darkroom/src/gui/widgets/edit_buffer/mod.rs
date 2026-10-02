//! Cross-frame state behind the buffered text fields (`inline_rename`, the node value editors,
//! the preferences path fields), and the one rule that decides what an edit frame meant.
//!
//! Every such field commits on Enter or on focus leaving it, and cancels on Escape. Escape blurs
//! too, so palantir reports `lost_focus` on the same frame as `cancelled`; [`DraftOutcome::of`]
//! is the single place that tests the cancel first.

use palantir::{TextEditResponse, Ui, WidgetId};

/// Cross-frame state for one buffered text edit.
#[derive(Default, Clone, Debug)]
pub(crate) struct EditBuffer {
    pub(crate) text: String,
    /// Focus as the field ended the last frame.
    was_focused: bool,
}

impl EditBuffer {
    /// Hand `id`'s retained text to `body` and put it back afterwards.
    ///
    /// A text widget wants a `&mut String` it can rewrite in place, and the
    /// row this buffer occupies is already holding one — lending it out for
    /// the call is what keeps a field that records every frame from
    /// allocating a fresh one every frame. The text is out of the state map
    /// for the duration, so `body` gets `ui` back mutably.
    pub(crate) fn with_text<R>(
        ui: &mut Ui,
        id: WidgetId,
        body: impl FnOnce(&mut Ui, &mut String) -> R,
    ) -> R {
        let mut text = std::mem::take(&mut ui.state_or_default::<Self>(id).text);
        let out = body(ui, &mut text);
        ui.state_or_default::<Self>(id).text = text;
        out
    }

    /// Whether the field is idle this frame, so its text may be refilled from the value it edits.
    ///
    /// Focus is resolved before the record pass, so on the frame focus leaves the field already
    /// reads unfocused. That frame is not idle: the draft has to survive it to be committed.
    pub(crate) const fn is_idle(&self, focused: bool) -> bool {
        !focused && !self.was_focused
    }

    /// Record the focus the field ends this frame with.
    pub(crate) const fn settle(&mut self, focused: bool) {
        self.was_focused = focused;
    }
}

/// What one frame of a buffered text field decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftOutcome {
    Editing,
    /// Enter, or focus left the field: write the draft.
    Commit,
    /// Escape: drop the draft.
    Cancel,
}

impl DraftOutcome {
    /// Escape wins: palantir reports `lost_focus` alongside `cancelled`, and testing the blur first
    /// would commit the text the user tried to throw away.
    pub(crate) const fn of(response: &TextEditResponse<'_>) -> Self {
        Self::from_signals(response.submitted, response.cancelled, response.lost_focus)
    }

    const fn from_signals(submitted: bool, cancelled: bool, lost_focus: bool) -> Self {
        if cancelled {
            Self::Cancel
        } else if submitted || lost_focus {
            Self::Commit
        } else {
            Self::Editing
        }
    }
}

#[cfg(test)]
mod tests;
