use glam::DVec2;
use imaginarium::Buffer2;

use crate::io::image::cfa::CfaType;
use crate::io::image::flat_gain::FlatGain;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::registration::transform::{Transform, WarpTransform};

/// A node is the mean gain of its colour within half a step, its excluded photosites left out.
/// - A mono 9×9 flat of 0.5 has a 3×3 grid of gain 2. One photosite at (4, 4) floored at 0.1 is
///   excluded: node (1, 1) reads its 24 neighbours' 2; included it would read
///   (24·2 + 10)/25 = 2.32.
/// - An RGGB flat of 0.5 red, 1 green and 0.25 blue gives each colour its own grid: 2, 1 and 4 at
///   every node, the corner nodes from the photosites their clipped window holds.
#[test]
fn a_node_is_its_colours_mean_gain() {
    let size = Size2us::new(9, 9);
    let mut pixels = vec![0.5; size.pixel_count()];
    pixels[4 * 9 + 4] = 0.1;
    let divisor = Buffer2::new(9, 9, pixels);
    let flagged = FlatGain::of_divisor(&divisor, &CfaType::Mono, |index| index == 4 * 9 + 4);
    let unflagged = FlatGain::of_divisor(&divisor, &CfaType::Mono, |_| false);
    for (x, y) in [(0.0, 0.0), (4.0, 4.0), (8.0, 4.0)] {
        assert_eq!(flagged.at(0, x, y), 2.0, "({x}, {y})");
    }
    assert_eq!(unflagged.at(0, 4.0, 4.0), (58.0f64 / 25.0) as f32);
    assert_eq!(unflagged.at(0, 0.0, 0.0), 2.0);

    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    let mosaic = Size2us::new(8, 8);
    let divisor = Buffer2::new(
        8,
        8,
        (0..mosaic.pixel_count())
            .map(
                |index| match cfa.color_at(Vec2us::new(index % 8, index / 8)) {
                    0 => 0.5,
                    1 => 1.0,
                    _ => 0.25,
                },
            )
            .collect(),
    );
    let gain = FlatGain::of_divisor(&divisor, &cfa, |_| false);
    for (colour, expected) in [(0, 2.0), (1, 1.0), (2, 4.0)] {
        for (x, y) in [(0.0, 0.0), (4.0, 4.0), (8.0, 0.0), (2.0, 6.0)] {
            assert_eq!(
                gain.at(colour, x, y),
                expected,
                "colour {colour} at ({x}, {y})"
            );
        }
    }
}

/// A flat whose gain rises as `1 + x/8` across a mono 9×9 frame: the node at x = 4 averages the
/// symmetric window 2..=6 to 1.5, the edge nodes their clipped windows, 1.125 over 0..=2 and 1.875
/// over 6..=8. Between nodes the gain is their line, 1.3125 at x = 2 and 1.6875 at x = 6; past the
/// grid it holds the outermost node. Warped by a shift of 4 pixels, output node 0 reads the source
/// at x = 4 and output node 1 the source's last node; the last output node, past the source, holds
/// that too. Each gain is `1/f` of an f32 `f`, within 1e-7 of the line.
#[test]
fn the_gain_is_bilinear_between_nodes_and_moves_with_the_warp() {
    let size = Size2us::new(9, 9);
    let divisor = Buffer2::new(
        9,
        9,
        (0..size.pixel_count())
            .map(|index| 1.0 / (1.0 + (index % 9) as f32 / 8.0))
            .collect(),
    );
    let gain = FlatGain::of_divisor(&divisor, &CfaType::Mono, |_| false);
    let close = |actual: f32, expected: f32, what: &str| {
        assert!(
            (actual - expected).abs() <= 1e-6,
            "{what}: {actual} against {expected}"
        );
    };
    for (x, expected) in [
        (0.0, 1.125),
        (2.0, 1.3125),
        (4.0, 1.5),
        (6.0, 1.6875),
        (8.0, 1.875),
        (20.0, 1.875),
        (-3.0, 1.125),
    ] {
        close(gain.at(0, x, 4.0), expected, &format!("x = {x}"));
    }

    let shifted = gain.warped(
        &WarpTransform::new(Transform::translation(DVec2::new(4.0, 0.0))),
        size,
    );
    assert_eq!(shifted.size(), size);
    for (x, expected) in [(0.0, 1.5), (4.0, 1.875), (8.0, 1.875)] {
        close(shifted.at(0, x, 0.0), expected, &format!("warped x = {x}"));
    }
}
