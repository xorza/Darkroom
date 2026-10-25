use common::Buffer2;
use glam::DVec2;

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flat_gain::gain_bins::GainBins;
use crate::image::flat_gain::gain_key::GainKey;
use crate::image::flat_gain::{FlatGain, FlatGainRecord};
use crate::image::frame_desc::MAX_SIDE;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use common::{Size2us, Vec2us};

/// A node is the mean gain of its color within half a step, its excluded photosites left out.
/// - A mono 9×9 flat of 0.5 has a 3×3 grid of gain 2. One photosite at (4, 4) floored at 0.1 is
///   excluded: node (1, 1) reads its 24 neighbours' 2; included it would read
///   (24·2 + 10)/25 = 2.32.
/// - An RGGB flat of 0.5 red, 1 green and 0.25 blue gives each color its own grid: 2, 1 and 4 at
///   every node, the corner nodes from the photosites their clipped window holds.
#[test]
fn a_node_is_its_colors_mean_gain() {
    let size = Size2us::new(9, 9);
    let mut pixels = vec![0.5; size.pixel_count()];
    pixels[4 * 9 + 4] = 0.1;
    let divisor = Buffer2::new((9, 9), pixels);
    let flagged = FlatGain::of_divisor(&divisor, &CfaType::Mono, |idx| idx == 4 * 9 + 4);
    let unflagged = FlatGain::of_divisor(&divisor, &CfaType::Mono, |_| false);
    for (x, y) in [(0.0, 0.0), (4.0, 4.0), (8.0, 4.0)] {
        assert_eq!(flagged.at(0, x, y), 2.0, "({x}, {y})");
    }
    assert_eq!(unflagged.at(0, 4.0, 4.0), (58.0f64 / 25.0) as f32);
    assert_eq!(unflagged.at(0, 0.0, 0.0), 2.0);

    let cfa = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let divisor = Buffer2::from_fn((8, 8), |x, y| match cfa.slot_at(Vec2us::new(x, y)) {
        0 => 0.5,
        1 => 1.0,
        _ => 0.25,
    });
    let gain = FlatGain::of_divisor(&divisor, &cfa, |_| false);
    for (color, expected) in [(0, 2.0), (1, 1.0), (2, 4.0)] {
        for (x, y) in [(0.0, 0.0), (4.0, 4.0), (8.0, 0.0), (2.0, 6.0)] {
            assert_eq!(
                gain.at(color, x, y),
                expected,
                "color {color} at ({x}, {y})"
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
    let divisor = Buffer2::from_fn((9, 9), |x, _| 1.0 / (1.0 + x as f32 / 8.0));
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

    let shift = WarpTransform::new(Transform::translation(DVec2::new(4.0, 0.0)));
    let shifted = gain.warped(|p| shift.apply(p), size);
    assert_eq!(shifted.size(), size);
    for (x, expected) in [(0.0, 1.5), (4.0, 1.875), (8.0, 1.875)] {
        close(shifted.at(0, x, 0.0), expected, &format!("warped x = {x}"));
    }

    // Three channels of 3 × 3 nodes, each its own ramp down the rows, warped by 4 pixels down:
    // every output node of every channel reads its own channel one node row lower, exactly, and
    // the last row, past the source, holds the source's last.
    let ramps = (1..=3)
        .map(|channel| Buffer2::from_fn((3, 3), |i, j| (10 * channel + 3 * j + i) as f32))
        .collect();
    let gain = FlatGain::from_nodes(size, ramps);
    let down = WarpTransform::new(Transform::translation(DVec2::new(0.0, 4.0)));
    let warped = gain.warped(|p| down.apply(p), size);
    for channel in 0..3 {
        for (i, j) in [(0, 0), (2, 0), (1, 1), (0, 2), (2, 2)] {
            let (x, y) = ((4 * i) as f32, (4 * j) as f32);
            assert_eq!(
                warped.at(channel, x, y).to_bits(),
                gain.at(channel, x, (y + 4.0).min(8.0)).to_bits(),
                "channel {channel}, node ({i}, {j})"
            );
        }
    }
}

/// A record reads back as a gain only when its size is an image's: a zero side has no grid and an
/// overflowing one no pixel count, and both are refused before either is computed. A 5×5 record of
/// one channel has a 2×2 grid, and reads back with the four nodes it stored.
#[test]
fn a_record_of_no_image_size_is_refused() {
    let record = |width, height, nodes: usize| FlatGainRecord {
        width,
        height,
        nodes: vec![vec![1.5; nodes]],
        bins: vec![GainBins::of(&mut [1.5])],
    };
    for (width, height) in [(0, 5), (5, 0), (usize::MAX, 2), (MAX_SIDE + 1, 1)] {
        assert!(
            FlatGain::try_from(record(width, height, 4)).is_err(),
            "{width}×{height}"
        );
    }
    assert!(FlatGain::try_from(record(5, 5, 3)).is_err());
    let gain = FlatGain::try_from(record(5, 5, 4)).unwrap();
    assert_eq!(gain.nodes[0].pixels(), &[1.5; 4]);
}

/// A gain's key is the blake3 digest of its size, its channel count and its nodes, each value
/// little-endian, whatever the machine: a 9×9 mono gain of 3×3 nodes hashes the three sizes as
/// `u64` and the 9 nodes as `f32`. An equal gain has the key, one node apart another, and the key
/// reads back from its 64 hex digits.
#[test]
fn a_gain_is_known_by_its_little_endian_digest() {
    let divisor = Buffer2::from_fn((9, 9), |x, y| 0.5 + 0.02 * (x * 9 + y) as f32);
    let gain = FlatGain::of_divisor(&divisor, &CfaType::Mono, |_| false);
    let mut bytes: Vec<u8> = [9u64, 9, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
    bytes.extend(gain.planes().flatten().flat_map(|node| node.to_le_bytes()));
    let expected = blake3::hash(&bytes).to_hex().to_string();
    assert_eq!(gain.key().to_string(), expected);
    assert_eq!(GainKey::from_hex(&expected), Some(gain.key()));
    assert_eq!(GainKey::from_hex("x"), None);

    let equal = FlatGain::of_divisor(&divisor, &CfaType::Mono, |_| false);
    assert_eq!(equal.key(), gain.key());
    let mut other = divisor;
    other.pixels_mut()[0] = 0.25;
    let other = FlatGain::of_divisor(&other, &CfaType::Mono, |_| false);
    assert_ne!(other.key(), gain.key());
}
