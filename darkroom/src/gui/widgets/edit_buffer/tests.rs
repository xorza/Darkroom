use super::*;

/// Every combination of the three signals: Escape cancels whatever else fired, and only Enter or a
/// blur commits.
#[test]
fn escape_wins_and_only_enter_or_blur_commits() {
    use DraftOutcome::{Cancel, Commit, Editing};
    for (submitted, cancelled, lost_focus, expected) in [
        (false, false, false, Editing),
        (true, false, false, Commit),
        (false, false, true, Commit),
        (true, false, true, Commit),
        (false, true, false, Cancel),
        (false, true, true, Cancel),
        (true, true, false, Cancel),
        (true, true, true, Cancel),
    ] {
        assert_eq!(
            DraftOutcome::from_signals(submitted, cancelled, lost_focus),
            expected,
            "submitted={submitted} cancelled={cancelled} lost_focus={lost_focus}"
        );
    }
}

/// Idle means unfocused now *and* at the end of the last frame: the frame focus leaves keeps the
/// draft, every later unfocused frame may refill it.
#[test]
fn the_blur_frame_is_not_idle() {
    let mut buf = EditBuffer::default();
    assert!(buf.is_idle(false), "never focused");
    buf.settle(false);
    assert!(!buf.is_idle(true), "focused");
    buf.settle(true);
    assert!(
        !buf.is_idle(false),
        "focus just left: the draft must survive"
    );
    buf.settle(false);
    assert!(buf.is_idle(false), "unfocused a frame later");
}
