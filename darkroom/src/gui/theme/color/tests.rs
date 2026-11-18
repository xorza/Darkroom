use super::*;

#[test]
fn toward_blends_the_hue_and_leaves_alpha_alone() {
    let a = RgbaF32::new(1.0, 0.0, 0.5, 0.8);
    let b = RgbaF32::new(0.0, 1.0, 0.5, 0.1);
    assert_eq!(toward(a, b, 0.0), a);
    // t = 1 lands on `b`'s rgb but keeps `a`'s alpha, where a lerp would take `b`'s 0.1.
    let full = toward(a, b, 1.0);
    assert_eq!((full.r, full.g, full.b, full.a), (0.0, 1.0, 0.5, 0.8));
    // Hand-computed midpoint: rgb (0.5, 0.5, 0.5), alpha still 0.8. A premultiplied blend, as
    // palantir's lerp is, would give (8/9, 1/9, 0.5): `a`'s 0.8 outweighs `b`'s 0.1.
    let mid = toward(a, b, 0.5);
    assert_eq!((mid.r, mid.g, mid.b, mid.a), (0.5, 0.5, 0.5, 0.8));
}
