//! [`EditBuffer`]: the cross-frame state behind the node value editors' buffered text fields.

use palantir::prelude::*;

/// Cross-frame state for one buffered text edit.
#[derive(Default, Clone, Debug)]
pub(crate) struct EditBuffer {
    pub(crate) text: String,
    /// Focus as the field ended the last frame.
    was: Focus,
}

/// Whether a field holds the keyboard focus.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    #[default]
    Elsewhere,
    Held,
}

impl Focus {
    /// The focus of the field `id` as `ui` resolved it this frame.
    pub(crate) fn of(ui: &Ui, id: WidgetId) -> Self {
        if ui.focus() == Some(id) {
            Self::Held
        } else {
            Self::Elsewhere
        }
    }
}

impl EditBuffer {
    /// Whether the field is idle this frame, so its text may be refilled from the value it edits.
    ///
    /// Focus is resolved before the record pass, so on the frame focus leaves the field already
    /// reads unfocused. That frame is not idle: the draft has to survive it to be committed.
    pub(crate) const fn is_idle(&self, focus: Focus) -> bool {
        matches!((focus, self.was), (Focus::Elsewhere, Focus::Elsewhere))
    }

    /// Record the focus the field ends this frame with.
    pub(crate) const fn settle(&mut self, focus: Focus) {
        self.was = focus;
    }
}

#[cfg(test)]
mod tests;
