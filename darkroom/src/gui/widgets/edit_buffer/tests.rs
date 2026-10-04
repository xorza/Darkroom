use super::*;

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
