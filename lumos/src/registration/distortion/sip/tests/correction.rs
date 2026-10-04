use super::*;

/// At the reference point every SIP monomial is zero, so the correction is exactly nothing.
#[test]
fn correct_at_reference_point_is_identity() {
    let sip = fit_field(&barrel(), 3);
    assert_eq!(sip.correct(barrel().centre), barrel().centre);
}

/// An order-3 fit of a cubic field recovers it: the correction at any point is the field's
/// displacement there, to [`EXACT_FIT_PX`] — on the grid, off it, and at the corners, under an
/// identity, a translation and a similarity, for barrel and pincushion fields about different
/// centres. Up to affine the fit target `J⁻¹·(t − T(r))` is the displacement itself.
#[test]
fn an_order_3_fit_recovers_a_cubic_field() {
    let pincushion = RadialField::new(DVec2::new(512.0, 384.0), -5e-8);
    let fields = [
        barrel(),
        pincushion,
        RadialField {
            transform: Transform::translation(DVec2::new(10.0, 5.0)),
            start: 100,
            extent: 900,
            ..barrel()
        },
        RadialField {
            transform: Transform::similarity(DVec2::new(30.0, -20.0), 0.2, 1.02),
            ..pincushion
        },
    ];
    for field in fields {
        let sip = fit_field(&field, 3);
        for p in [
            DVec2::new(700.0, 300.0),
            DVec2::new(123.4, 876.5),
            DVec2::new(0.0, 0.0),
            DVec2::new(1000.0, 1000.0),
        ] {
            let miss = (sip.correct(p) - p - field.displacement(p)).length();
            assert!(miss <= EXACT_FIT_PX, "{field:?} at {p:?}: {miss:e}");
        }
    }
}

/// The fit is linear in the targets' displacement: the barrel's correction negated is the
/// pincushion's of the same strength, and five times the barrel is five times its correction —
/// to the rounding of each fit.
#[test]
fn the_fit_is_linear_in_the_field() {
    let one = fit_field(&barrel(), 3);
    let negated = fit_field(
        &RadialField {
            k: -1e-7,
            ..barrel()
        },
        3,
    );
    let fivefold = fit_field(
        &RadialField {
            k: 5e-7,
            ..barrel()
        },
        3,
    );
    for p in [DVec2::new(800.0, 200.0), DVec2::new(0.0, 0.0)] {
        let base = one.correct(p) - p;
        let opposite = negated.correct(p) - p;
        let scaled = fivefold.correct(p) - p;
        assert!((base + opposite).length() <= 2.0 * EXACT_FIT_PX, "{p:?}");
        assert!(
            (scaled - 5.0 * base).length() <= 6.0 * EXACT_FIT_PX,
            "{p:?}"
        );
    }
}

/// The analytic Jacobian of `correct` against a central difference, for an order-5 fit of a barrel
/// field at points across it. The difference with step `h` = 1e-2 px has truncation error
/// `h²/6·|c'''|` — the correction's third derivative is at most `6·k` = 6e-7 per px² here, so 1e-11
/// — and rounding error `u·|p|/h` ≈ 2e-11 at `|p|` ≈ 1000; 1e-9 holds the sum.
#[test]
fn jacobian_is_the_derivative_of_correct() {
    let sip = fit_field(
        &RadialField {
            step: 50,
            ..barrel()
        },
        5,
    );
    let h = 1e-2;
    for p in [
        DVec2::new(500.0, 500.0),
        DVec2::new(120.0, 870.0),
        DVec2::new(950.0, 40.0),
    ] {
        let jacobian = sip.jacobian(p);
        for (axis, step) in [DVec2::new(h, 0.0), DVec2::new(0.0, h)]
            .into_iter()
            .enumerate()
        {
            let difference = (sip.correct(p + step) - sip.correct(p - step)) / (2.0 * h);
            let column = jacobian.col(axis);
            assert!(
                (column - difference).length() < 1e-9,
                "at {p:?} axis {axis}: {column:?} against {difference:?}"
            );
        }
    }
}
