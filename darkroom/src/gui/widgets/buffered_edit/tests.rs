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
