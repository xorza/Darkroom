//! [`EditBuffer`]: the cross-frame state behind the node value editors' buffered text fields.

/// Cross-frame state for one buffered text edit.
#[derive(Default, Clone, Debug)]
pub(crate) struct EditBuffer {
    pub(crate) text: String,
    /// Focus as the field ended the last frame.
    was_focused: bool,
}

impl EditBuffer {
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

#[cfg(test)]
mod tests;
