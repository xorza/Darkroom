#[cfg(feature = "real-data")]
mod real_data;

use crate::image_ops::color_calibration::*;
use crate::internals::assertions::assert_close_slice;
use crate::internals::images::{gray_image as gray, rgb_image as rgb};
use crate::internals::prelude::*;

/// A green background at 0.3 over red and blue at 0.1, and a white star 0.4 above every channel.
/// The flat backgrounds are their own medians, exactly; neutralizing moves green down by 0.2 to the
/// darkest, which leaves every pixel neutral and the star 0.4 above it. Red and blue do not move;
/// green rounds twice — the shift, half an ulp of 0.2, and the sum, half an ulp of at most 0.7 —
/// so it meets red to under ε.
#[test]
fn neutralize_equalizes_backgrounds_and_makes_image_neutral() {
    let r = [vec![0.1; 7], vec![0.5, 0.5]].concat();
    let g = [vec![0.3; 7], vec![0.7, 0.7]].concat();
    let b = [vec![0.1; 7], vec![0.5, 0.5]].concat();
    let mut img = rgb(Size2us::new(3, 3), r.clone(), g, b.clone());

    let before = channel_backgrounds(&img);
    assert_eq!((before.r, before.g, before.b), (0.1, 0.3, 0.1));

    NeutralizeBackground.apply(&mut img).unwrap();

    assert_eq!(img.channel(0).pixels(), r.as_slice(), "red stays");
    assert_eq!(img.channel(2).pixels(), b.as_slice(), "blue stays");
    assert_close_slice!(img.channel(1).pixels(), r, f32::EPSILON, "green meets red");
}

/// Average-neutral clamps green to the red/blue mean and nothing else: (0.2, 0.6, 0.2) takes the
/// mean 0.2, exact; (0.5, 0.3, 0.5) is below its mean and stays.
#[test]
fn scnr_average_neutral_clamps_only_green_excess() {
    let mut img = rgb(
        Size2us::new(2, 1),
        vec![0.2, 0.5],
        vec![0.6, 0.3],
        vec![0.2, 0.5],
    );
    Scnr::average_neutral(1.0).apply(&mut img).unwrap();
    assert_eq!(img.channel(0).pixels(), &[0.2, 0.5], "R unchanged");
    assert_eq!(img.channel(1).pixels(), &[0.2, 0.3]);
    assert_eq!(img.channel(2).pixels(), &[0.2, 0.5], "B unchanged");
}

/// Additive mask, `G′ = G·(1 − amount)·(1 − m) + m·G` with `m = min(1, R + B)`, by hand. On
/// (0.2, 0.6, 0.2), `m = 0.4`: amount 0 keeps 0.6, ½ gives 0.6·½·0.6 + 0.4·0.6 = 0.42, and 1 gives
/// 0.24. Where `R + B` reaches 1 the mask saturates: `m = 1` keeps green whatever the amount,
/// exactly, as `1 − m` is 0. Otherwise six roundings of terms no larger than G, half an ulp each:
/// 3ε·G. Red and blue never move.
#[test]
fn scnr_additive_mask_hand_computed() {
    for ((r, g, b), amount, expected) in [
        ((0.2f32, 0.6f32, 0.2f32), 0.0f32, 0.6f64),
        ((0.2, 0.6, 0.2), 0.5, 0.42),
        ((0.2, 0.6, 0.2), 1.0, 0.24),
        ((0.7, 0.6, 0.5), 0.5, 0.6),
        ((0.7, 0.6, 0.5), 1.0, 0.6),
    ] {
        let mut img = rgb(Size2us::new(1, 1), vec![r], vec![g], vec![b]);
        Scnr::additive_mask(amount).apply(&mut img).unwrap();
        let green = img.channel(1).pixels()[0];
        assert_close!(
            green,
            expected,
            3.0 * f32::EPSILON * g,
            "{r} {g} {b} at {amount}"
        );
        assert_eq!(
            (img.channel(0).pixels()[0], img.channel(2).pixels()[0]),
            (r, b)
        );
        if r + b >= 1.0 {
            assert_eq!(green, g, "saturated mask at {amount}");
        }
    }
}

/// Every protection at its full strength and halfway, by hand, each blending
/// `G′ = (1 − amount)·G + amount·G_full`. On (0.1, 0.6, 0.3): Average Neutral clamps to the mean 0.2,
/// Maximum Neutral to the larger 0.3; Additive Mask keeps `min(1, 0.4) = 0.4` of green, 0.24, and
/// Maximum Mask `max(0.1, 0.3) = 0.3` of it, 0.18. Halfway is the midpoint of 0.6 and each: 0.4,
/// 0.45, 0.42 and 0.39. Amount 0 changes nothing. Each value takes three roundings of terms no
/// larger than G: 2ε·G. Red and blue never move.
#[test]
fn every_protection_blends_by_its_amount() {
    let (r, g, b) = (0.1f32, 0.6f32, 0.3f32);
    for (name, scnr, full, half) in [
        (
            "average neutral",
            Scnr::average_neutral as fn(f32) -> Scnr,
            0.2,
            0.4,
        ),
        ("maximum neutral", Scnr::maximum_neutral, 0.3, 0.45),
        ("additive mask", Scnr::additive_mask, 0.24, 0.42),
        ("maximum mask", Scnr::maximum_mask, 0.18, 0.39),
    ] {
        for (amount, expected) in [(0.0f32, 0.6f64), (0.5, half), (1.0, full)] {
            let mut img = rgb(Size2us::new(1, 1), vec![r], vec![g], vec![b]);
            scnr(amount).apply(&mut img).unwrap();
            assert_close!(
                img.channel(1).pixels()[0],
                expected,
                2.0 * f32::EPSILON * g,
                "{name} at {amount}"
            );
            assert_eq!(
                (img.channel(0).pixels()[0], img.channel(2).pixels()[0]),
                (r, b)
            );
        }
        let mut img = rgb(Size2us::new(1, 1), vec![r], vec![g], vec![b]);
        assert!(scnr(1.5).apply(&mut img).is_err(), "{name}");
    }
}

#[test]
fn color_ops_are_noops_on_grayscale() {
    let mut g = gray(Size2us::new(2, 1), vec![0.3, 0.7]);
    NeutralizeBackground.apply(&mut g).unwrap();
    Scnr::average_neutral(1.0).apply(&mut g).unwrap();
    assert_eq!(
        g.channel(0).pixels(),
        vec![0.3, 0.7],
        "grayscale left unchanged"
    );
}
