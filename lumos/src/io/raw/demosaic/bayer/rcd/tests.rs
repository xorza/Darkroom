use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::bayer::rcd::{EPS, MIN_SIGNED_DENOMINATOR_RATIO, estimate_green};
use crate::testing::prelude::*;

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
    let expected = 0.5 * (additive + canonical);
    let actual = estimate_green(neighbor_green, center_lpf, same_color_lpf);

    assert!((actual - expected).abs() < 1e-6);
}

#[derive(Debug, Clone, Copy)]
enum Neighborhood {
    Edge,
    Impulse,
    ChromaticStar,
    Noise,
}

fn neighborhood_value(neighborhood: Neighborhood, channel: usize, pos: Vec2us) -> f32 {
    match neighborhood {
        Neighborhood::Edge => {
            if pos.x < 4 {
                [0.7, 0.4, 0.2][channel]
            } else {
                [0.1, 0.3, 0.8][channel]
            }
        }
        Neighborhood::Impulse => {
            let impulse = if pos.x == 4 && pos.y == 2 { 0.9 } else { 0.0 };
            [0.05 + impulse, 0.08, 0.03][channel]
        }
        Neighborhood::ChromaticStar => {
            let dx = pos.x as f32 - 3.5;
            let dy = pos.y as f32 - 2.0;
            let profile = (-0.5 * (dx * dx + dy * dy)).exp();
            0.02 + [0.9, 0.5, 0.2][channel] * profile
        }
        Neighborhood::Noise => {
            let sample = (pos.x * 17 + pos.y * 29 + channel * 11) % 31;
            0.02 + sample as f32 / 62.0
        }
    }
}

fn neighborhood_lpf(cfa: &[f32], width: usize, y: usize, x: usize) -> f32 {
    let index = y * width + x;
    cfa[index]
        + 0.5 * (cfa[index - width] + cfa[index + width] + cfa[index - 1] + cfa[index + 1])
        + 0.25
            * (cfa[index - width - 1]
                + cfa[index - width + 1]
                + cfa[index + width - 1]
                + cfa[index + width + 1])
}

fn reference_signed_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    let neighbor_green = f64::from(neighbor_green);
    let center_lpf = f64::from(center_lpf);
    let same_color_lpf = f64::from(same_color_lpf);
    let epsilon = f64::from(EPS);
    let numerator = neighbor_green * (center_lpf + center_lpf);
    let denominator = epsilon + center_lpf + same_color_lpf;
    if center_lpf >= 0.0 && same_color_lpf >= 0.0 {
        return (numerator / denominator) as f32;
    }

    let scale = epsilon + center_lpf.abs() + same_color_lpf.abs();
    let transition = f64::from(MIN_SIGNED_DENOMINATOR_RATIO) * scale;
    if denominator.abs() >= transition {
        return (numerator / denominator) as f32;
    }

    let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
    let t = denominator.abs() / transition;
    let curve = t * (3.0 - 2.0 * t);
    let ratio_weight = t * curve;
    let weighted_ratio = numerator * denominator.signum() * curve / transition;
    (additive * (1.0 - ratio_weight) + weighted_ratio) as f32
}

#[test]
fn signed_scene_neighborhoods_match_f64_reference_across_zero() {
    const WIDTH: usize = 7;
    const HEIGHT: usize = 5;
    const CENTER_X: usize = 4;
    const SAME_COLOR_X: usize = 2;
    const Y: usize = 2;

    let pattern = CfaPattern::Rggb;
    for neighborhood in [
        Neighborhood::Edge,
        Neighborhood::Impulse,
        Neighborhood::ChromaticStar,
        Neighborhood::Noise,
    ] {
        let cfa: Vec<f32> = (0..HEIGHT)
            .flat_map(|y| {
                (0..WIDTH).map(move |x| {
                    neighborhood_value(
                        neighborhood,
                        pattern.color_at(Vec2us::new(x, y)),
                        Vec2us::new(x, y),
                    )
                })
            })
            .collect();
        let center_lpf = neighborhood_lpf(&cfa, WIDTH, Y, CENTER_X);
        let same_color_lpf = neighborhood_lpf(&cfa, WIDTH, Y, SAME_COLOR_X);
        let neighbor_green = cfa[Y * WIDTH + 3];

        for (label, anchor) in [("center", center_lpf), ("same-color", same_color_lpf)] {
            for target in [-2.0 * EPS, -EPS, -0.5 * EPS, 0.0, 0.5 * EPS, EPS, 2.0 * EPS] {
                let pedestal = (target - anchor) * 0.25;
                let shifted_neighbor = neighbor_green + pedestal;
                let shifted_center = center_lpf + 4.0 * pedestal;
                let shifted_same_color = same_color_lpf + 4.0 * pedestal;
                let actual = estimate_green(shifted_neighbor, shifted_center, shifted_same_color);
                let expected =
                    reference_signed_green(shifted_neighbor, shifted_center, shifted_same_color);
                let tolerance = 2e-6 * expected.abs().max(1.0);
                assert!(
                    (actual - expected).abs() <= tolerance,
                    "{neighborhood:?} {label} LPF={target}: expected {expected}, got {actual}"
                );
            }
        }
    }
}
