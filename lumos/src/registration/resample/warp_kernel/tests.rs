use common::Buffer2;
use glam::DVec2;

use crate::image::flags::pixel_flags::Reach;
use crate::math::lanczos::lanczos_lut::{LANCZOS_LUT_RESOLUTION, LanczosOrder};
use crate::registration::resample::source_pos::{AxisPos, SourcePos};
use crate::registration::resample::warp_kernel::internals::bicubic_kernel;
use crate::registration::resample::warp_kernel::{
    Filter, TapAxis, TapRange, WarpKernel, largest_singular_value,
};
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use crate::simd::isa_tier::IsaTier;
use crate::simd::{Isa, Kernel};
use common::Size2us;

const FILTERS: [Filter; 5] = [
    Filter::Bilinear,
    Filter::Bicubic,
    Filter::Lanczos(LanczosOrder::Two),
    Filter::Lanczos(LanczosOrder::Three),
    Filter::Lanczos(LanczosOrder::Four),
];

/// [`WarpKernel::axis`] on whichever Isa a tier runs.
#[derive(Debug)]
struct Axis<'a> {
    kernel: WarpKernel,
    frac: f32,
    axis: &'a mut TapAxis,
}

impl Kernel for Axis<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let at = AxisPos {
            cell: 10,
            frac: self.frac,
        };
        self.kernel.axis(isa, at, self.axis);
    }
}

/// The scalar definition of one tap's weight: the distance `|t − frac|` as f32 rounds it, then the
/// table read, the Catmull-Rom polynomial or the tent at the stretched distance, and zero at or
/// past the reach.
fn scalar_weight(kernel: WarpKernel, t: i32, frac: f32) -> f32 {
    let stretch = kernel.stretch;
    let distance = (t as f32 - frac).abs();
    if distance >= kernel.reach {
        return 0.0;
    }
    match kernel.shape.filter() {
        Filter::Lanczos(order) => order
            .lut()
            .at(distance * (LANCZOS_LUT_RESOLUTION as f32 / stretch)),
        Filter::Bicubic => bicubic_kernel(distance * (1.0 / stretch)),
        Filter::Bilinear => (1.0 - distance * (1.0 / stretch)).max(0.0),
    }
}

/// The vector weights are the scalar definitions bit for bit, on every tier, for every filter at
/// five stretches — two inside the edge tolerance of 1 — and at every 1/256 of a pixel, the
/// fraction just below 1, and 1 itself; and the slots of a vector past the window weigh nothing.
#[test]
fn vector_weights_are_the_scalar_definitions() {
    let fracs: Vec<f32> = (0..256)
        .map(|k| k as f32 / 256.0)
        .chain([1.0 - f32::EPSILON / 2.0, 1.0])
        .collect();
    let mut axis = TapAxis::default();
    for filter in FILTERS {
        for stretch in [1.0, 1.0f32.next_up(), 1.000_05, 1.37, 2.5] {
            let kernel = WarpKernel::new(filter, stretch);
            for tier in IsaTier::supported() {
                for &frac in &fracs {
                    tier.run(Axis {
                        kernel,
                        frac,
                        axis: &mut axis,
                    });
                    let TapRange { first, count } = kernel.taps(frac);
                    assert_eq!(axis.start, 10 + first);
                    assert_eq!(axis.weights().len(), count);
                    for (t, &weight) in (first..).zip(axis.weights()) {
                        assert_eq!(
                            weight.to_bits(),
                            scalar_weight(kernel, t, frac).to_bits(),
                            "{tier} {filter:?} ×{stretch} at {frac}, tap {t}"
                        );
                    }
                    let past = count.next_multiple_of(8);
                    assert!(
                        axis.padded_from(count)[..past - count]
                            .iter()
                            .all(|&weight| weight == 0.0),
                        "{tier} {filter:?} ×{stretch} at {frac}: a slot past the window weighs"
                    );
                }
            }
        }
    }
}

/// Unstretched, a window is the kernel's `2·radius` taps from `cell − (radius − 1)` at any
/// fraction. Stretched, it is the taps strictly within the reach: Lanczos3 at 1.5 reaches
/// `(3 − ε)·1.5` = 4.4996, so at fraction 0.25 the taps run from −4 (distance 4.25) to 4 (distance
/// 3.75), nine of them, since −5 and 5 sit at 5.25 and 4.75; at fraction 0.5 the taps −4 and 5 sit
/// at 4.5 and drop out, leaving −3 to 4. The window reach covers both: `⌈4.4996⌉ = 5` after the
/// cell and 4 before it.
///
/// The reach stops short of the radius by the filter's edge tolerance, where the kernel falls
/// under `2⁻²⁷`: for Lanczos3 `ε = 3·2⁻¹³·⁵/(1 + 2⁻¹³·⁵)` = 2.5892e-4, for Catmull-Rom
/// `2⁻¹³` = 1.2207e-4, for Bilinear `2⁻²⁷`.
/// - One f32 step above 1, `1 + 2⁻²³`, Lanczos3 reaches 2.99974 and Catmull-Rom 1.99988, so each
///   keeps its window reach at 1. Bilinear reaches `1 + 2⁻²³` itself: its tent rises linearly
///   from its edge, so the tap at distance 1 weighs `1 − 1/(1 + 2⁻²³)` ≈ 1.2e-7, over `2⁻²⁷`, and
///   the window grows to (1, 2).
/// - Lanczos3 at 1.0002 reaches `2.99974·1.0002` = 3.00034, past 3: (3, 4).
/// - Lanczos3 at 1.00005 reaches `2.99974·1.00005` = 2.99989. At fraction 5e-5 the taps run from
///   −2 to 2: tap 3, at 2.99995, lies past the reach although the kernel there,
///   `L(2.99995/1.00005)` = `L(2.9998)` ≈ `(2e-4/3)²` = 4.4e-9, is not zero, so its slot weighs 0.
#[test]
fn a_window_holds_the_taps_within_the_stretched_radius() {
    let range = |kernel: WarpKernel, frac| {
        let TapRange { first, count } = kernel.taps(frac);
        (first, count)
    };
    let lanczos3 = Filter::Lanczos(LanczosOrder::Three);
    for frac in [0.0, 0.3, 1.0] {
        assert_eq!(range(WarpKernel::new(lanczos3, 1.0), frac), (-2, 6));
        assert_eq!(range(WarpKernel::new(Filter::Bilinear, 1.0), frac), (0, 2));
    }
    let stretched = WarpKernel::new(lanczos3, 1.5);
    assert_eq!(range(stretched, 0.25), (-4, 9));
    assert_eq!(range(stretched, 0.5), (-3, 8));
    let reach = |kernel: WarpKernel| {
        let Reach { before, after } = kernel.window_reach();
        (before, after)
    };
    assert_eq!(reach(WarpKernel::new(lanczos3, 1.0)), (2, 3));
    assert_eq!(reach(stretched), (4, 5));
    assert_eq!(reach(WarpKernel::new(Filter::Bilinear, 1.0)), (0, 1));

    let step_above = 1.0f32.next_up();
    assert_eq!(reach(WarpKernel::new(lanczos3, step_above)), (2, 3));
    assert_eq!(reach(WarpKernel::new(Filter::Bicubic, step_above)), (1, 2));
    assert_eq!(reach(WarpKernel::new(Filter::Bilinear, step_above)), (1, 2));
    assert_eq!(reach(WarpKernel::new(lanczos3, 1.0002)), (3, 4));

    let banded = WarpKernel::new(lanczos3, 1.000_05);
    let frac = 5e-5;
    assert_eq!(range(banded, frac), (-2, 5));
    let unmasked = LanczosOrder::Three
        .lut()
        .at((3.0 - frac) * (LANCZOS_LUT_RESOLUTION as f32 / 1.000_05));
    assert!(unmasked > 0.0, "premise: the kernel at tap 3 is {unmasked}");
    for tier in IsaTier::supported() {
        let mut axis = TapAxis::default();
        tier.run(Axis {
            kernel: banded,
            frac,
            axis: &mut axis,
        });
        assert_eq!(axis.padded_from(5)[0], 0.0, "{tier}");
    }
}

/// The stretch is the output-to-source Jacobian's largest singular value, held to 1.
///
/// - A similarity of scale 2 at any rotation has singular values 2 and 2: stretch 2.
/// - Scale 0.5 enlarges the frame: stretch 1, the kernel as it is.
/// - The affine map `[[3, 1], [0, 0.5]]` has `F = 9 + 1 + 0.25 = 10.25` and `det = 1.5`, so
///   `σ² = (10.25 + √(10.25² − 9))/2` = `(10.25 + √96.0625)/2` = `(10.25 + 9.8011)/2` = 10.0256,
///   σ = 3.16632.
/// - A homography of scale 0.5 at the origin grows its scale toward the far corner as its
///   denominator `1 − 10⁻³(x + y)` falls to 0.682, past 1 there (the corner still lands inside
///   the source, at (145.9, 87.2)); the corner is a node of the grid, so the stretch is that
///   corner's scale.
#[test]
fn the_stretch_is_the_largest_local_scale() {
    let size = Size2us::new(200, 120);
    let stretch = |transform: Transform| {
        let warp = WarpTransform::new(transform);
        let kernel = WarpKernel::for_frame(Filter::Bilinear, &warp, size);
        kernel.stretch
    };
    assert_eq!(
        stretch(Transform::similarity(DVec2::new(3.0, -2.0), 0.7, 2.0)),
        2.0
    );
    assert_eq!(
        stretch(Transform::similarity(DVec2::new(3.0, -2.0), 0.7, 0.5)),
        1.0
    );
    assert_eq!(stretch(Transform::identity()), 1.0);
    let sheared = stretch(Transform::affine([3.0, 1.0, 0.0, 0.0, 0.5, 0.0]));
    let expected = f64::midpoint(10.25, 96.0625f64.sqrt()).sqrt() as f32;
    assert_eq!(sheared, expected);
    assert!((sheared - 3.16632).abs() < 1e-5, "{sheared}");

    let homography = Transform::homography([0.5, 0.0, 0.0, 0.0, 0.5, 0.0, -1e-3, -1e-3]);
    let corner = DVec2::new(199.0, 119.0);
    let warp = WarpTransform::new(homography);
    let at_corner = largest_singular_value(warp.jacobian(corner));
    assert!(at_corner > 1.0, "{at_corner}");
    assert_eq!(stretch(homography), at_corner as f32);
}

/// Catmull-Rom (`A = −½`) by hand, every value dyadic and so exact in f32:
/// inner branch `(1.5|x| − 2.5)·x² + 1`, outer `((−0.5|x| + 2.5)·|x| − 4)·|x| + 2`.
/// `K(0.25)` = (0.375 − 2.5)·0.0625 + 1 = 0.8671875; `K(0.5)` = (0.75 − 2.5)·0.25 + 1 = 0.5625;
/// `K(0.75)` = (1.125 − 2.5)·0.5625 + 1 = 0.2265625; `K(1)` = 0 from both branches;
/// `K(1.25)` = (1.875·1.25 − 4)·1.25 + 2 = −0.0703125; `K(1.5)` = (1.75·1.5 − 4)·1.5 + 2 = −0.0625;
/// `K(1.75)` = (1.625·1.75 − 4)·1.75 + 2 = −0.0234375; `K(2)` = 0, and nothing beyond.
///
/// Both branches have slope `−½` at 1 (`4.5x² − 5x` and `−1.5x² + 5x − 4`), so across `1 ± 1e-4`
/// the kernel falls by 1e-4; a jump between the branches would show on top of that. The f32
/// evaluation of terms near 2.5 that cancel to 5e-5 rounds to a few ulps of 2.5, under 1e-6.
#[test]
fn bicubic_kernel_hand_values() {
    for (x, expected) in [
        (0.0, 1.0),
        (0.25, 0.867_187_5),
        (0.5, 0.5625),
        (0.75, 0.226_562_5),
        (1.0, 0.0),
        (1.25, -0.070_312_5),
        (1.5, -0.0625),
        (1.75, -0.023_437_5),
        (2.0, 0.0),
        (2.5, 0.0),
    ] {
        assert_eq!(bicubic_kernel(x), expected, "K({x})");
        assert_eq!(bicubic_kernel(-x), expected, "K(-{x})");
    }
    let fall = bicubic_kernel(1.0 - 1e-4) - bicubic_kernel(1.0 + 1e-4);
    assert!((fall - 1e-4).abs() < 1e-6, "{fall}");
}

/// The nearest pixel, a half rounding up: on `[[10, 20], [30, 40]]`, (0.4, 0.4) is pixel (0, 0),
/// (1.4, 0.4) is (1, 0), (0.4, 1.4) is (0, 1), and (0.5, 0) rounds up to (1, 0). In the rim past
/// the last centre it holds to the edge pixel: (1.45, −0.45) is (1, 0).
#[test]
fn nearest_rounds_a_half_up() {
    let input = Buffer2::new((2, 2), vec![10.0, 20.0, 30.0, 40.0]);
    let size = Size2us::new(2, 2);
    for (x, y, expected) in [
        (0.4, 0.4, 10.0),
        (1.4, 0.4, 20.0),
        (0.4, 1.4, 30.0),
        (1.4, 1.4, 40.0),
        (0.5, 0.0, 20.0),
        (1.45, -0.45, 20.0),
    ] {
        let pos = SourcePos::within(DVec2::new(x, y), size).unwrap();
        assert_eq!(
            input.pixels()[pos.nearest_idx(size)],
            expected,
            "({x}, {y})"
        );
    }
}
