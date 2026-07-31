//! Shared cross-frame core behind [`crate::gui::widgets::inline_rename`]
//! and `gui::node::value_editor`: a text buffer that survives
//! across frames in palantir's `StateMap`, plus detection of the exact
//! frame focus transitions `true → false` (the "blur edge") — the
//! conventional trigger to commit a text-field edit.
//!
//! The one thing callers can't share: a widget driven through
//! `Ui::request_focus` (`inline_rename`'s double-click swap from label
//! to editor) opens a gap of one or more frames between the request and
//! focus actually landing, during which a plain "focused last frame, not
//! now" check would misread "hasn't landed yet" as a blur.
//! [`EditBuffer::blur_edge`] arms its latch only once focus truly lands
//! and disarms it the instant a blur is reported, so it's safe for a
//! `request_focus`-driven caller and a plain click-to-focus one alike —
//! `value_editor` never calls `request_focus`, so the gap never opens
//! and the latch reduces to a last-frame focus register.

/// Cross-frame state for one in-progress buffered text edit.
#[derive(Default, Clone, Debug)]
pub(crate) struct EditBuffer {
    pub(crate) text: String,
    /// Arms once focus lands, disarms the instant a blur is reported —
    /// not a plain last-frame mirror, so a pending `request_focus`
    /// doesn't read as a blur before it lands (see module docs).
    focus_latch: bool,
}

impl EditBuffer {
    /// Advance the latch by one frame; returns whether this is the
    /// exact blur edge (focus was held since the latch last armed, and
    /// is gone now).
    pub(crate) fn blur_edge(&mut self, focused: bool) -> bool {
        let blurred = self.focus_latch && !focused;
        self.focus_latch = (self.focus_latch || focused) && !blurred;
        blurred
    }

    /// Force the latch closed outside of a blur — call when an edit
    /// session ends some other way (Enter, or Escape while still
    /// focused) or (re)starts via `request_focus`, so a stale armed
    /// latch can't misfire as a blur on the next frame or session.
    pub(crate) fn reset_latch(&mut self) {
        self.focus_latch = false;
    }
}

#[cfg(test)]
mod tests;
