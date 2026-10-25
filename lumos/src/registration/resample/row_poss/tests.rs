use glam::DVec2;

use crate::registration::distortion::sip::SipModel;
use crate::registration::distortion::sip::sip_order::SipOrder;
use crate::registration::resample::row_poss::RowPoss;
use crate::registration::resample::source_pos::SourcePos;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use common::Size2us;

/// Every row of an affine model or a homography is the bits `WarpTransform::apply` gives, split
/// by `SourcePos::within`, and positions past the footprint are `None`. A SIP row sums its
/// polynomial in another order, so it agrees to rounding instead: the correction here reaches
/// `1e-6·180³` ≈ 6 px, so the reordering moves it by a few 1e-15 px, and the f32 fraction of
/// the split, under 1, rounds to half its 6e-8 spacing — 1e-7 holds both.
#[test]
fn a_row_is_what_apply_gives_pixel_by_pixel() {
    let source = Size2us::new(300, 200);
    let center = DVec2::new(150.0, 100.0);
    let affine = Transform::similarity(DVec2::new(-12.5, 7.25), 0.03, 1.01);
    let mut reference = Vec::new();
    for y in (0..200).step_by(20) {
        for x in (0..300).step_by(20) {
            reference.push(DVec2::new(f64::from(x), f64::from(y)));
        }
    }
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&r| {
            let d = r - center;
            affine.apply(r + d * 1e-6 * d.length_squared())
        })
        .collect();
    let sip = SipModel {
        order: SipOrder::Three,
        origin: center,
    }
    .fitted_under(&affine, &reference, &target);
    let transforms = [
        WarpTransform::new(affine),
        WarpTransform::new(Transform::homography([
            1.02, 0.01, 30.0, -0.015, 0.98, -4.0, 4e-5, -3e-5,
        ])),
        WarpTransform::with_sip(affine, sip),
    ];
    let mut row = RowPoss::default();
    for transform in &transforms {
        let mut outside = 0;
        for y in [0, 57, 199] {
            row.fill(y, transform, source);
            assert_eq!(row.poss().len(), 300);
            for (x, &pos) in row.poss().iter().enumerate() {
                let exact = transform.apply(DVec2::new(x as f64, y as f64));
                let expected = SourcePos::within(exact, source);
                if transform.has_sip() {
                    assert_eq!(pos.is_some(), expected.is_some(), "x = {x}, y = {y}");
                    if let Some(p) = pos {
                        let split = DVec2::new(
                            f64::from(p.cell_x) + f64::from(p.fx),
                            f64::from(p.cell_y) + f64::from(p.fy),
                        );
                        assert!(
                            (split - exact).length() < 1e-7,
                            "x = {x}, y = {y}: {split:?} against {exact:?}"
                        );
                    }
                } else {
                    assert_eq!(pos, expected, "x = {x}, y = {y}");
                }
                outside += usize::from(pos.is_none());
            }
        }
        assert!(outside > 0, "the rows reach past the footprint");
    }
}
