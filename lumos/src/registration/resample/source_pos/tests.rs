use glam::DVec2;

use crate::registration::resample::source_pos::SourcePos;
use common::Size2us;

const SIZE: Size2us = Size2us::new(10, 6);

/// Inside the footprint a position splits into its floor and the exact remainder; the rim is
/// inclusive, and anything past it or not finite is outside.
#[test]
fn within_splits_inside_the_footprint_only() {
    let split = |x, y| SourcePos::within(DVec2::new(x, y), SIZE);
    assert_eq!(
        split(3.25, 4.75),
        Some(SourcePos {
            cell_x: 3,
            cell_y: 4,
            fx: 0.25,
            fy: 0.75
        })
    );
    assert_eq!(
        split(-0.5, 5.5),
        Some(SourcePos {
            cell_x: -1,
            cell_y: 5,
            fx: 0.5,
            fy: 0.5
        })
    );
    for (x, y) in [
        (-0.500_000_1, 1.0),
        (9.500_000_1, 1.0),
        (1.0, 5.500_000_1),
        (f64::NAN, 1.0),
        (1.0, f64::INFINITY),
    ] {
        assert_eq!(split(x, y), None, "({x}, {y})");
    }
}

/// The fraction keeps its precision where the position would not: at x = 6000.123, f32 spacing
/// is 4.9e-4, so a position narrowed first lands on 6000.123047 and loses 4.7e-5 of its
/// fraction, while the f64 split keeps 0.123 to f32's 7.5e-9 at that magnitude.
#[test]
fn the_fraction_is_split_before_it_is_narrowed() {
    let p = SourcePos::within(DVec2::new(6000.123, 2.0), Size2us::new(8000, 4)).unwrap();
    assert_eq!(p.cell_x, 6000);
    assert!((f64::from(p.fx) - 0.123).abs() < 1e-8, "{}", p.fx);
    let narrowed_first = 6000.123f32 - 6000.0;
    assert!((f64::from(narrowed_first) - 0.123).abs() > 4e-5);
}

/// The rim on either side snaps to the edge pixel's centre; the interior is untouched.
#[test]
fn clamping_holds_positions_to_pixel_centres() {
    let clamp = |x, y| {
        SourcePos::within(DVec2::new(x, y), SIZE)
            .unwrap()
            .clamped_to_centers(SIZE)
    };
    let at = |cell_x, fx, cell_y, fy| SourcePos {
        cell_x,
        cell_y,
        fx,
        fy,
    };
    assert_eq!(clamp(-0.25, 9.0 - 4.0), at(0, 0.0, 5, 0.0));
    assert_eq!(clamp(9.25, 0.5), at(9, 0.0, 0, 0.5));
    assert_eq!(clamp(4.5, 2.25), at(4, 0.5, 2, 0.25));
}
