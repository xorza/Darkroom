use super::*;

/// Idle means unfocused now *and* at the end of the last frame: the frame focus leaves keeps the
/// draft, every later unfocused frame may refill it.
#[test]
fn the_blur_frame_is_not_idle() {
    let mut buf = EditBuffer::default();
    assert!(buf.is_idle(Focus::Elsewhere), "never focused");
    buf.settle(Focus::Elsewhere);
    assert!(!buf.is_idle(Focus::Held), "focused");
    buf.settle(Focus::Held);
    assert!(
        !buf.is_idle(Focus::Elsewhere),
        "focus just left: the draft must survive"
    );
    buf.settle(Focus::Elsewhere);
    assert!(buf.is_idle(Focus::Elsewhere), "unfocused a frame later");
}
