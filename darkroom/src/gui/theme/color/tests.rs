use super::*;

#[test]
fn toward_blends_the_hue_and_leaves_alpha_alone() {
    let a = RgbaF32::new(1.0, 0.0, 0.5, 0.8);
    let b = RgbaF32::new(0.0, 1.0, 0.5, 0.1);
    assert_eq!(toward(a, b, 0.0), a);
    // t = 1 lands on `b`'s rgb but keeps `a`'s alpha — the whole point of
    // this wrapper over the plain `Animatable::lerp`, which would have taken
    // `b`'s 0.1 along with it.
    let full = toward(a, b, 1.0);
    assert_eq!((full.r, full.g, full.b, full.a), (0.0, 1.0, 0.5, 0.8));
    assert!(
        (Animatable::lerp(a, b, 1.0).a - b.a).abs() < 1e-6,
        "the bare lerp carries alpha to the far end"
    );
    // Hand-computed midpoint: rgb (0.5, 0.5, 0.5), alpha still 0.8.
    let mid = toward(a, b, 0.5);
    assert_eq!((mid.r, mid.g, mid.b, mid.a), (0.5, 0.5, 0.5, 0.8));
}

/// The lift moves each channel 28% of its way to white and keeps alpha: a zero channel lands on
/// 0 + (1 − 0)·0.28 = 0.28 exactly, and a full one stays at 1.
#[test]
fn hover_lift_moves_each_channel_toward_white_and_keeps_alpha() {
    let lifted = hover_lift(RgbaF32::new(0.0, 1.0, 0.0, 0.5));
    assert_eq!(
        (lifted.r, lifted.g, lifted.b, lifted.a),
        (0.28, 1.0, 0.28, 0.5)
    );
    assert_eq!(hover_lift(RgbaF32::WHITE), RgbaF32::WHITE);
}
