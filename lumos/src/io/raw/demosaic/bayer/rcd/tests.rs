use crate::io::raw::demosaic::bayer::rcd::{EPS, MIN_SIGNED_DENOMINATOR_RATIO, estimate_green};

fn canonical_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    neighbor_green * (center_lpf + center_lpf) / (EPS + center_lpf + same_color_lpf)
}

#[test]
fn well_conditioned_green_estimate_matches_canonical_ratio() {
    for neighbor_green in [-0.75, 0.0, 1.5] {
        for center_lpf in [0.0, EPS * 0.5, EPS, 4.0] {
            for same_color_lpf in [0.0, EPS * 0.5, EPS, 8.0] {
                assert_eq!(
                    estimate_green(neighbor_green, center_lpf, same_color_lpf),
                    canonical_green(neighbor_green, center_lpf, same_color_lpf)
                );
            }
        }
    }

    for (neighbor_green, center_lpf, same_color_lpf) in [(0.25, 1.0, -0.5), (-0.75, -4.0, -2.0)] {
        assert_eq!(
            estimate_green(neighbor_green, center_lpf, same_color_lpf),
            canonical_green(neighbor_green, center_lpf, same_color_lpf)
        );
    }
}

#[test]
fn cancelling_green_estimate_has_the_additive_limit() {
    let center_lpf = 1.0;
    let same_color_lpf = -(EPS + center_lpf);
    let actual = estimate_green(0.25, center_lpf, same_color_lpf);
    let expected = 0.25 + (1.0 - same_color_lpf) / 8.0;
    assert_eq!(actual, expected);
}

#[test]
fn cancelling_green_estimate_blends_halfway_at_half_the_condition_limit() {
    let neighbor_green = 0.25;
    let center_lpf = 1.0;
    let condition = 0.5 * MIN_SIGNED_DENOMINATOR_RATIO;
    let same_color_lpf = -(EPS + center_lpf) * (1.0 - condition) / (1.0 + condition);
    let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
    let canonical = canonical_green(neighbor_green, center_lpf, same_color_lpf);
    let expected = f32::midpoint(additive, canonical);
    let actual = estimate_green(neighbor_green, center_lpf, same_color_lpf);

    assert!((actual - expected).abs() < 1e-6);
}

/// The estimate is continuous where it switches from the ratio to the blend. At the switch,
/// `t = |denominator| / transition` is 1 and the blend puts all its weight on the ratio, so
/// denominators just outside and just inside give values that differ by the forms' slopes times
/// the step. In `t` the ratio's slope is the ratio itself (under 8·|green| by the condition limit)
/// and the blend's is the additive estimate, so a step of 2e-5 moves either by under
/// 2e-5 · (8·|green| + |additive|).
#[test]
fn the_green_estimate_is_continuous_across_the_switch() {
    let condition = f64::from(MIN_SIGNED_DENOMINATOR_RATIO);
    let epsilon = f64::from(EPS);
    for (green, center) in [(0.25f64, 1.0f64), (-0.5, 2.0), (1.5, 0.3)] {
        // The negative same-colour LPF that puts the denominator at `t` times the transition:
        // EPS + c + s = t·R·(EPS + c − s), solved for s.
        let same_color_at = |t: f64| {
            (t * condition * (epsilon + center) - (epsilon + center)) / (1.0 + t * condition)
        };
        let at = |t: f64| estimate_green(green as f32, center as f32, same_color_at(t) as f32);
        let (outside, inside) = (at(1.0 + 1e-5), at(1.0 - 1e-5));
        let additive = green + (center - same_color_at(1.0)) * 0.125;
        let bound = 2e-5 * (8.0 * green.abs() + additive.abs());
        assert!(
            f64::from((outside - inside).abs()) < bound,
            "green {green}, center {center}: {outside} outside vs {inside} inside"
        );
    }
}
