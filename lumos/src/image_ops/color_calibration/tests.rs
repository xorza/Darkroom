use crate::image_ops::color_calibration::*;
use crate::image_ops::op::OpError;
use crate::image_ops::test_support::{
    channel_samples as channel, gray_image as gray, rgb_image as rgb,
};

#[test]
fn neutralize_equalizes_backgrounds_and_makes_image_neutral() {
    // Green background elevated (0.3) vs red/blue (0.1); a white star adds an equal 0.4 to every
    // channel, so it reads greenish before. 7 background pixels + 2 star pixels (3x3).
    let r = [vec![0.1; 7], vec![0.5, 0.5]].concat();
    let g = [vec![0.3; 7], vec![0.7, 0.7]].concat();
    let b = [vec![0.1; 7], vec![0.5, 0.5]].concat();
    let mut img = rgb(3, 3, r, g, b);

    let before = channel_backgrounds(&img);
    assert!(
        (before.r - 0.1).abs() < 1e-6
            && (before.g - 0.3).abs() < 1e-6
            && (before.b - 0.1).abs() < 1e-6,
        "green background elevated before: {before:?}"
    );

    NeutralizeBackground.apply(&mut img).unwrap();

    // Backgrounds all shifted to the darkest channel (0.1).
    let after = channel_backgrounds(&img);
    assert!(
        (after.r - 0.1).abs() < 1e-6
            && (after.g - 0.1).abs() < 1e-6
            && (after.b - 0.1).abs() < 1e-6,
        "backgrounds equalized to min: {after:?}"
    );
    // The whole image is now neutral (R=G=B per pixel) — the green pedestal is gone from the star too.
    let (rc, gc, bc) = (channel(&img, 0), channel(&img, 1), channel(&img, 2));
    for i in 0..rc.len() {
        assert!(
            (gc[i] - rc[i]).abs() < 1e-6 && (bc[i] - rc[i]).abs() < 1e-6,
            "pixel {i} neutral: r={} g={} b={}",
            rc[i],
            gc[i],
            bc[i]
        );
    }
    // Signal above background is preserved: the star is 0.4 above the (now 0.1) background.
    assert!(
        (gc[7] - 0.5).abs() < 1e-6,
        "star green = bg + signal = 0.1 + 0.4: {}",
        gc[7]
    );
}

#[test]
fn scnr_average_neutral_clamps_only_green_excess() {
    // px0: green above (R+B)/2 -> clamped; px1: green below it -> untouched. R and B never change.
    let mut img = rgb(2, 1, vec![0.2, 0.5], vec![0.6, 0.3], vec![0.2, 0.5]);
    Scnr::average_neutral().apply(&mut img).unwrap();
    let (r, g, b) = (channel(&img, 0), channel(&img, 1), channel(&img, 2));
    assert!(
        (g[0] - 0.2).abs() < 1e-6,
        "G clamped to (R+B)/2 = 0.2, got {}",
        g[0]
    );
    assert!(
        (g[1] - 0.3).abs() < 1e-6,
        "G below average untouched, got {}",
        g[1]
    );
    assert_eq!(r, vec![0.2, 0.5], "R unchanged");
    assert_eq!(b, vec![0.2, 0.5], "B unchanged");
}

#[test]
fn scnr_additive_mask_amount_zero_noop_and_full_hand_computed() {
    // amount = 0 is a no-op.
    let mut img0 = rgb(1, 1, vec![0.2], vec![0.6], vec![0.2]);
    Scnr::additive_mask(0.0).apply(&mut img0).unwrap();
    assert!(
        (channel(&img0, 1)[0] - 0.6).abs() < 1e-6,
        "amount 0 is a no-op"
    );

    // amount = 1: m = min(1, R+B) = min(1, 0.4) = 0.4; G' = G·0·(1−m) + m·G = 0.4·0.6 = 0.24.
    let mut img1 = rgb(1, 1, vec![0.2], vec![0.6], vec![0.2]);
    Scnr::additive_mask(1.0).apply(&mut img1).unwrap();
    assert!(
        (channel(&img1, 1)[0] - 0.24).abs() < 1e-6,
        "additive mask at full strength: {}",
        channel(&img1, 1)[0]
    );
}

#[test]
fn color_ops_are_noops_on_grayscale() {
    let mut g = gray(2, 1, vec![0.3, 0.7]);
    NeutralizeBackground.apply(&mut g).unwrap();
    Scnr::average_neutral().apply(&mut g).unwrap();
    assert_eq!(channel(&g, 0), vec![0.3, 0.7], "grayscale left unchanged");
}

#[test]
fn scnr_rejects_out_of_range_amount() {
    let mut img = rgb(1, 1, vec![0.2], vec![0.6], vec![0.2]);
    let err = Scnr::additive_mask(1.5).apply(&mut img).unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.contains("SCNR amount must be in")),
        "expected an InvalidConfig SCNR amount error, got {err:?}"
    );
}
