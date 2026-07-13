//! Shared cross-frame core behind [`crate::gui::widgets::inline_rename`]
//! and [`crate::gui::node::value_editor`]: a text buffer that survives
//! across frames in aperture's `StateMap`, plus detection of the exact
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
mod tests {
    use super::*;

    #[test]
    fn blur_edge_fires_once_on_the_true_to_false_transition() {
        let mut buf = EditBuffer::default();
        // Idle: never focused, never a blur.
        assert!(!buf.blur_edge(false));
        assert!(!buf.blur_edge(false));
        // Focus lands: not itself a blur.
        assert!(!buf.blur_edge(true));
        assert!(!buf.blur_edge(true));
        // The exact frame focus is lost is the blur edge...
        assert!(buf.blur_edge(false));
        // ...and staying unfocused afterward doesn't re-report it.
        assert!(!buf.blur_edge(false));
        assert!(!buf.blur_edge(false));
    }

    #[test]
    fn blur_edge_ignores_the_request_focus_gap() {
        // Mirrors inline_rename: `reset_latch` at session start, then
        // one or more frames where `request_focus` hasn't landed yet
        // (still reads unfocused) before it actually does.
        let mut buf = EditBuffer::default();
        buf.reset_latch();
        assert!(!buf.blur_edge(false), "gap frame must not read as blur");
        assert!(
            !buf.blur_edge(false),
            "a longer gap must not read as blur either"
        );
        // Focus lands for real.
        assert!(!buf.blur_edge(true));
        // Now a real blur is reported correctly.
        assert!(buf.blur_edge(false));
    }

    #[test]
    fn reset_latch_forces_a_non_blur_exit() {
        // Mirrors Enter (or Escape) committed while still focused: the
        // caller ends the session itself, so the latch must not carry
        // an armed blur into whatever comes next.
        let mut buf = EditBuffer::default();
        assert!(!buf.blur_edge(true));
        buf.reset_latch();
        // Without the reset this would report a blur (latch was
        // armed); with it, the next unfocused frame is clean.
        assert!(!buf.blur_edge(false));
    }

    #[test]
    fn blur_edge_matches_a_plain_last_frame_register_without_a_gap() {
        // For a caller that never opens a request_focus gap (value_editor),
        // the latch must reduce exactly to `was_focused = focused`, checked
        // by hand-computing the reference formula alongside the latch.
        let mut buf = EditBuffer::default();
        let mut was_focused = false;
        for focused in [false, true, true, false, false, true, false] {
            let blurred = buf.blur_edge(focused);
            assert_eq!(blurred, was_focused && !focused);
            was_focused = focused;
        }
    }
}
