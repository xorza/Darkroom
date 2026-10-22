use crate::registration::resample::ringing_clamp::{LobeSums, LobeWeights, RingingClamp};
use crate::registration::resample::warp_config::ClampingThreshold;

const LOBES: LobeWeights = LobeWeights {
    positive: 1.6,
    negative: -0.6,
};

fn light(positive: f32, negative: f32) -> LobeSums {
    LobeSums {
        positive,
        negative,
        below_zero: 0.0,
    }
}

/// Each branch of PixInsight's rule at threshold 0.3, on weights `wₚ = 1.6`, `wₙ = −0.6`
/// (total 1), by hand:
/// - `r = 0.5/2 = 0.25 ≤ 0.3`: the plain kernel, `(2 − 0.5)/1 = 1.5`, without asking for the
///   lobe weights.
/// - `r = 1/2 = 0.5`: `c = 1 − (0.2/0.7)² = 45/49`, so `(2 − 45/49)/(1.6 − 0.6·45/49)` =
///   `(53/49)/(51.4/49)` = `53/51.4`.
/// - `r = 2.5/2 ≥ 1`: the positive lobes' mean, `2/1.6 = 1.25`.
/// - nothing under the positive lobes: 0.
/// - a part below zero passes through the total weight: `−0.3/1` added to each.
///
/// The soft branch rounds through a handful of f32 operations on values near 1, so 1e-6.
#[test]
fn each_branch_of_the_rule_by_hand() {
    let clamp = RingingClamp::new(ClampingThreshold::new(0.3).unwrap());
    let no_lobes = || -> LobeWeights { panic!("the plain branch needs no lobe weights") };
    assert_eq!(clamp.sample(light(2.0, -0.5), 1.0, no_lobes), 1.5);
    let soft = clamp.sample(light(2.0, -1.0), 1.0, || LOBES);
    assert!((soft - 53.0 / 51.4).abs() < 1e-6, "{soft}");
    assert_eq!(clamp.sample(light(2.0, -2.5), 1.0, || LOBES), 1.25);
    assert_eq!(clamp.sample(light(0.0, -2.5), 1.0, || LOBES), 0.0);
    let below = LobeSums {
        below_zero: -0.3,
        ..light(2.0, -0.5)
    };
    assert_eq!(clamp.sample(below, 1.0, no_lobes), 1.5 - 0.3);
}

/// At threshold 1 the soft branch is never taken: `r = 0.95` is the plain kernel, `0.1/1`, and
/// `r = 1` already the positive lobes' mean. A flat field of 3 under lobes `1.6` and `−0.9`
/// has `r = 0.5625`, past 0.3, and reads 3 back to rounding: its sums scale with its weights.
#[test]
fn the_threshold_ends_and_a_flat_field() {
    let open = RingingClamp::new(ClampingThreshold::new(1.0).unwrap());
    let plain = open.sample(light(2.0, -1.9), 1.0, || {
        panic!("r = 0.95 is plain at threshold 1")
    });
    assert!((plain - 0.1).abs() < 1e-6, "{plain}");
    assert_eq!(open.sample(light(2.0, -2.0), 1.0, || LOBES), 1.25);

    let lobes = LobeWeights {
        positive: 1.6,
        negative: -0.9,
    };
    let flat = RingingClamp::new(ClampingThreshold::new(0.3).unwrap()).sample(
        light(3.0 * 1.6, 3.0 * -0.9),
        0.7,
        || lobes,
    );
    assert!((flat - 3.0).abs() < 1e-5, "{flat}");
}
