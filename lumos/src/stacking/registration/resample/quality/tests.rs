use crate::stacking::combine::pixel_coverage::PixelCoverage;
use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::resample::kernel;
use crate::stacking::registration::resample::quality;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::testing::prelude::*;

const TOL: f32 = 1e-5;
const INTERPOLATION_METHODS: [InterpolationMethod; 6] = [
    InterpolationMethod::Nearest,
    InterpolationMethod::Bilinear,
    InterpolationMethod::Bicubic,
    InterpolationMethod::Lanczos2,
    InterpolationMethod::Lanczos3,
    InterpolationMethod::Lanczos4,
];

#[test]
fn warp_coverage_nearest_identity_is_all_ones() {
    let size = Size2us::new(8, 8);
    let wt = WarpTransform::new(Transform::identity());
    let maps = quality::internals::maps(size, &wt, InterpolationMethod::Nearest);
    for &c in maps.coverage.pixels() {
        assert!(
            (c - 1.0).abs() < TOL,
            "nearest identity coverage should be 1.0, got {c}"
        );
    }
    // Nearest takes one tap at full weight, so its interpolation adds no variance anywhere.
    for &c in maps.confidence.pixels() {
        assert!(
            (c - 1.0).abs() < TOL,
            "nearest identity confidence should be 1.0, got {c}"
        );
    }
}

#[test]
fn warp_coverage_fully_outside_is_zero() {
    let size = Size2us::new(8, 8);
    // Source translated far outside the image: every kernel tap is out of bounds.
    let wt = WarpTransform::new(Transform::translation(DVec2::new(1000.0, 1000.0)));
    let cov = quality::internals::maps(size, &wt, InterpolationMethod::Bilinear).coverage;
    for &c in cov.pixels() {
        assert_eq!(c, 0.0, "fully-outside coverage must be 0, got {c}");
    }
}

#[test]
fn warp_coverage_bilinear_edge_is_partial() {
    let size = Size2us::new(8, 8);
    // Output (0,4) maps to src (-0.5, 4.0): the 2×2 bilinear footprint straddles the left
    // edge — taps at x=-1 (out, weight 0.5) and x=0 (in, weight 0.5) → coverage 0.5.
    let wt = WarpTransform::new(Transform::translation(DVec2::new(-0.5, 0.0)));
    let cov = quality::internals::maps(size, &wt, InterpolationMethod::Bilinear).coverage;
    let edge = cov.pixels()[4 * size.width];
    assert!(
        (edge - 0.5).abs() < TOL,
        "left-edge bilinear coverage should be 0.5, got {edge}"
    );
    // An interior output pixel maps fully in bounds → coverage 1.0.
    let interior = cov.pixels()[4 * size.width + 4];
    assert!(
        (interior - 1.0).abs() < TOL,
        "interior coverage should be 1.0, got {interior}"
    );
}

#[test]
fn bilinear_quality_has_hand_computed_support_and_confidence() {
    let dims = Size2us::new(8, 8);
    let interior = quality::quality_at(Vec2::new(0.5, 4.0), dims, InterpolationMethod::Bilinear);
    assert!((interior.coverage - 1.0).abs() < TOL);
    // Coefficients [0.5, 0.5] have variance gain 0.5, so inverse variance is 2.
    assert!((interior.confidence - 2.0).abs() < TOL);

    let edge = quality::quality_at(Vec2::new(-0.5, 4.0), dims, InterpolationMethod::Bilinear);
    assert!((edge.coverage - 0.5).abs() < TOL);
    // Renormalization leaves the sole in-bounds coefficient equal to one.
    assert!((edge.confidence - 1.0).abs() < TOL);
}

#[test]
fn source_footprint_boundary_is_inclusive() {
    let dims = Size2us::new(8, 6);
    for position in [
        Vec2::new(-0.5, 2.0),
        Vec2::new(7.5, 2.0),
        Vec2::new(3.0, -0.5),
        Vec2::new(3.0, 5.5),
    ] {
        assert!(
            kernel::source_footprint_contains(position, dims),
            "{position:?}"
        );
    }
    for position in [
        Vec2::new(-0.5001, 2.0),
        Vec2::new(7.5001, 2.0),
        Vec2::new(3.0, -0.5001),
        Vec2::new(3.0, 5.5001),
    ] {
        assert!(
            !kernel::source_footprint_contains(position, dims),
            "{position:?}"
        );
        for method in INTERPOLATION_METHODS {
            let quality = quality::quality_at(position, dims, method);
            assert_eq!(quality.coverage, 0.0, "{method:?} at {position:?}");
            assert_eq!(quality.confidence, 0.0, "{method:?} at {position:?}");
        }
    }
}

/// The interior fast path is a shortcut, not an approximation: where every tap has data behind it,
/// skipping the per-tap bounds test and the coverage ratio has to give bit-identical results to
/// clipping against the source bounds. Asserted exactly — a fast path that merely rounds the same
/// way most of the time would put a second, quietly different confidence into the weight plane.
#[test]
fn the_interior_fast_path_matches_clipping_bit_for_bit() {
    let size = Size2us::new(20, 16);
    let mut interior_positions = 0;
    // Nearest has no separable window, and the Lanczos family takes its interior sums from
    // `lanczos_interior_sums` instead — `tabulated_interior_sums_track_the_computed_ones` is that
    // path's check. What is left here is the two kernels that still sum their own weights.
    for method in [InterpolationMethod::Bilinear, InterpolationMethod::Bicubic] {
        // Well inside the border band on both axes, stepped off the pixel grid so the sub-pixel
        // phase varies rather than repeating one set of weights.
        let mut x = 6.0;
        while x < 13.0 {
            let mut y = 5.0;
            while y < 11.0 {
                let taps = match method {
                    InterpolationMethod::Bicubic => {
                        quality::SeparableTaps::bicubic(Vec2::new(x, y))
                    }
                    _ => quality::SeparableTaps::bilinear(Vec2::new(x, y)),
                };
                assert!(
                    taps.is_interior(size),
                    "{method:?} at ({x}, {y}) is not interior, so it proves nothing"
                );
                interior_positions += 1;

                let fast = taps.interior_quality();
                let clipped_x = taps.clipped_x(size);
                let clipped_y = taps.clipped_y(size);
                let clipped = quality::SampleQuality {
                    coverage: quality::separable_coverage(clipped_x, clipped_y),
                    confidence: quality::separable_confidence(clipped_x.in_sums, clipped_y.in_sums),
                };
                assert_eq!(
                    fast.coverage.to_bits(),
                    clipped.coverage.to_bits(),
                    "{method:?} at ({x}, {y}): coverage {} against {}",
                    fast.coverage,
                    clipped.coverage
                );
                assert_eq!(
                    fast.confidence.to_bits(),
                    clipped.confidence.to_bits(),
                    "{method:?} at ({x}, {y}): confidence {} against {}",
                    fast.confidence,
                    clipped.confidence
                );
                y += 0.13;
            }
            x += 0.17;
        }
    }
    assert!(
        interior_positions > 800,
        "only {interior_positions} interior positions were compared"
    );
}

/// The Lanczos interior path reads its tap-weight sums from a table indexed by the fractional
/// offset instead of summing `2a` LUT reads per axis. The table has to answer what the summation
/// would have.
///
/// Exactly, at every fraction the LUT itself distinguishes — those are the entries. Between them,
/// `offset + f` rounds to f32 before scaling, so a fraction within an ulp of an index boundary can
/// take the neighbouring entry; that is one table step, and the bound below is what the combine's
/// weight plane inherits. The probes deliberately include those boundaries: a uniform sweep alone
/// finds no disagreement at all for Lanczos2, and would leave the tolerance untested.
#[test]
fn tabulated_interior_sums_track_the_computed_ones() {
    // Measured worst across all three widths is 6.5e-4 on a per-axis sum.
    const TABULATED_TOLERANCE: f32 = 1e-3;
    for a in [2usize, 3, 4] {
        let table = quality::lanczos_interior_sums(a);

        // On the table's own grid the two must agree bit for bit — anything else means the index
        // does not name the entry it was built from.
        for index in (0..=quality::LANCZOS_LUT_RESOLUTION).step_by(37) {
            let f = index as f32 / quality::LANCZOS_LUT_RESOLUTION as f32;
            let mut weights = [0.0; quality::MAX_TAPS];
            quality::lanczos_weights(a, f, &mut weights);
            let computed = quality::AxisSums::of(&weights[..2 * a]);
            let tabulated = table[quality::fraction_index(f)];
            assert_eq!(
                (tabulated.signed.to_bits(), tabulated.square.to_bits()),
                (computed.signed.to_bits(), computed.square.to_bits()),
                "Lanczos{a} at grid fraction {f}"
            );
        }

        // Off the grid, within one table step — including fractions an ulp either side of an index
        // boundary, which is exactly where `offset + f` can round into the neighbouring entry.
        let mut probes: Vec<f32> = (0..20_000).map(|step| step as f32 / 20_000.0).collect();
        for index in 0..quality::LANCZOS_LUT_RESOLUTION {
            let boundary = (index as f32 + 0.5) / quality::LANCZOS_LUT_RESOLUTION as f32;
            probes.push(boundary);
            probes.push(f32::from_bits(boundary.to_bits() + 1));
            probes.push(f32::from_bits(boundary.to_bits() - 1));
        }
        for f in probes {
            let mut weights = [0.0; quality::MAX_TAPS];
            quality::lanczos_weights(a, f, &mut weights);
            let computed = quality::AxisSums::of(&weights[..2 * a]);
            let tabulated = table[quality::fraction_index(f)];
            for (tabulated, computed) in [
                (tabulated.signed, computed.signed),
                (tabulated.square, computed.square),
            ] {
                let relative = (tabulated - computed).abs() / computed.abs();
                assert!(
                    relative <= TABULATED_TOLERANCE,
                    "Lanczos{a} at {f}: tabulated {tabulated} against computed {computed}"
                );
            }
        }
    }
}

/// The pairing `combine` gates on, checked at the end that produces it: a warped pixel has support
/// and interpolation confidence together or neither, for every method, everywhere a kernel can
/// straddle a border. `PixelCoverage::contributes` is the consumer's rule, so the survivors it
/// admits are also the ones whose confidence has to be usable as an inverse-variance weight and as
/// `source_noise_variance`'s divisor — hence the floor asserted on them.
#[test]
fn support_and_confidence_vanish_together_across_every_border() {
    let dims = Size2us::new(24, 18);
    // Stepping by an awkward fraction sweeps sub-pixel phase instead of landing on pixel centres,
    // and the span reaches a full kernel radius past both borders on each axis.
    const STEP: f32 = 0.157;
    for method in INTERPOLATION_METHODS {
        let radius = method.kernel_radius() as f32 + 1.0;
        let mut lowest_survivor = f32::INFINITY;
        let mut partial_support_seen = false;
        let mut uncovered_seen = false;
        let mut x = -radius;
        while x <= dims.width as f32 + radius {
            let mut y = -radius;
            while y <= dims.height as f32 + radius {
                let quality = quality::quality_at(Vec2::new(x, y), dims, method);
                assert_eq!(
                    quality.coverage > 0.0,
                    quality.confidence > 0.0,
                    "{method:?} at ({x}, {y}): coverage {} against confidence {}",
                    quality.coverage,
                    quality.confidence
                );
                uncovered_seen |= quality.coverage == 0.0;
                if PixelCoverage::new(quality.coverage).contributes() {
                    lowest_survivor = lowest_survivor.min(quality.confidence);
                    partial_support_seen |= quality.coverage < 1.0;
                }
                y += STEP;
            }
            x += STEP;
        }
        // The pairing holds trivially away from a border, so prove the sweep crossed one.
        assert!(
            uncovered_seen && lowest_survivor.is_finite(),
            "{method:?}: the sweep stayed on one side of the border"
        );
        // Nearest is binary by construction — a single tap, in or out — while every other kernel has
        // taps straddling the border and so a band of partial support between the two.
        assert_eq!(
            partial_support_seen,
            method != InterpolationMethod::Nearest,
            "{method:?}: partially supported pixels seen = {partial_support_seen}"
        );
        // Measured worst case is 0.365 (Lanczos4 at a corner), and 1.0 for Nearest. A kernel change
        // that drops below this makes a survivor's weight, and the reciprocal normalization takes of
        // it, larger than anything seen so far.
        assert!(
            lowest_survivor >= 0.36,
            "{method:?}: a contributing pixel had confidence {lowest_survivor}, under the 0.36 floor"
        );
    }
}

#[test]
fn coverage_is_continuous_and_monotonic_across_left_border() {
    let dims = Size2us::new(32, 32);
    for method in INTERPOLATION_METHODS {
        let radius = method.kernel_radius() as i32;
        let mut previous = 0.0;
        for integer in -radius..=radius {
            let coverage =
                quality::quality_at(Vec2::new(integer as f32 + 0.37, 16.0), dims, method).coverage;
            assert!(
                coverage + 1e-6 >= previous,
                "{method:?}: coverage decreased from {previous} to {coverage} at x={integer}"
            );
            previous = coverage;
        }
        assert!((previous - 1.0).abs() < TOL, "{method:?}: {previous}");

        if method != InterpolationMethod::Nearest {
            let left = quality::quality_at(Vec2::new(-1e-4, 16.0), dims, method).coverage;
            let right = quality::quality_at(Vec2::new(1e-4, 16.0), dims, method).coverage;
            assert!(
                (left - right).abs() < 1e-3,
                "{method:?}: discontinuity across x=0: {left} vs {right}"
            );
        }
    }
}
