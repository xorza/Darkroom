use glam::DVec2;

use crate::io::image::pixel_flags::Reach;
use crate::math::size2us::Size2us;
use crate::registration::resample::kernel::internals::bicubic_kernel;
use crate::registration::resample::kernel::warp_kernel::{
    Filter, TapAxis, TapRange, WarpKernel, largest_singular_value,
};
use crate::registration::resample::kernel::{LANCZOS_LUT_RESOLUTION, LanczosOrder};
use crate::registration::transform::{Transform, WarpTransform};
use crate::simd::tier::Tier;
use crate::simd::{Isa, Kernel};

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
        self.kernel.axis(isa, 10, self.frac, self.axis);
    }
}

/// The scalar definition of one tap's weight: the distance `|t − frac|` as f32 rounds it, then the
/// table read, the Catmull-Rom polynomial or the tent at the stretched distance.
fn scalar_weight(filter: Filter, stretch: f32, t: i32, frac: f32) -> f32 {
    let distance = (t as f32 - frac).abs();
    match filter {
        Filter::Lanczos(order) => order
            .lut()
            .at(distance * (LANCZOS_LUT_RESOLUTION as f32 / stretch)),
        Filter::Bicubic => bicubic_kernel(distance * (1.0 / stretch)),
        Filter::Bilinear => (1.0 - distance * (1.0 / stretch)).max(0.0),
    }
}

/// The vector weights are the scalar definitions bit for bit, on every tier, for every filter at
/// three stretches and at every 1/256 of a pixel, the fraction just below 1, and 1 itself; and the
/// slots of a vector past the window weigh nothing.
#[test]
fn vector_weights_are_the_scalar_definitions() {
    let fracs: Vec<f32> = (0..256)
        .map(|k| k as f32 / 256.0)
        .chain([1.0 - f32::EPSILON / 2.0, 1.0])
        .collect();
    let mut axis = TapAxis::default();
    for filter in FILTERS {
        for stretch in [1.0, 1.37, 2.5] {
            let kernel = WarpKernel::new(filter, stretch);
            for tier in Tier::supported() {
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
                            scalar_weight(filter, stretch, t, frac).to_bits(),
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
/// fraction. Stretched, it is the taps strictly within `radius · stretch`: Lanczos3 at 1.5
/// reaches 4.5, so at fraction 0.25 the taps run from −4 (distance 4.25) to 4 (distance 3.75),
/// nine of them, since −5 and 5 sit at 5.25 and 4.75; at fraction 0.5 the taps −4 and 5 sit at
/// exactly 4.5 and drop out, leaving −3 to 4. The window reach covers both: `⌈4.5⌉ = 5` after the
/// cell and 4 before it.
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
