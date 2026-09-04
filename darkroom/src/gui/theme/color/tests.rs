use super::*;

#[test]
fn toward_blends_the_hue_and_leaves_alpha_alone() {
    let a = RgbaF32::new(1.0, 0.0, 0.5, 0.8);
    let b = RgbaF32::new(0.0, 1.0, 0.5, 0.1);
    assert_eq!(toward(a, b, 0.0), a);
    // t = 1 lands on `b`'s rgb but keeps `a`'s alpha — the whole point of
    // this wrapper over the plain `RgbaF32::lerp`, which would have taken
    // `b`'s 0.1 along with it.
    let full = toward(a, b, 1.0);
    assert_eq!((full.r, full.g, full.b, full.a), (0.0, 1.0, 0.5, 0.8));
    assert!(
        (a.lerp(b, 1.0).a - b.a).abs() < 1e-6,
        "the bare lerp carries alpha to the far end"
    );
    // Hand-computed midpoint: rgb (0.5, 0.5, 0.5), alpha still 0.8.
    let mid = toward(a, b, 0.5);
    assert_eq!((mid.r, mid.g, mid.b, mid.a), (0.5, 0.5, 0.5, 0.8));
}
