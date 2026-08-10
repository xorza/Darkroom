//! What the calibration masters hold that is not frame data.
//!
//! The tiered loader's frame budgeting is covered by `combine`'s `mem_budget`; this pins the
//! scratch that budget does not model, and that nothing else would notice growing.

use imaginarium::Buffer2;

use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::cosmic_ray::internals::{
    CONCURRENT_MASKS, MONO_SCRATCH_PLANES, mono_scratch_floats, new_cr_mask,
};
use crate::stacking::calibration_masters::cosmic_ray::{CosmicRayConfig, reject_cosmic_rays};
use crate::testing::cfa_from_plane;

/// Cosmic-ray detection holds three full-frame masks at its peak, one bit per pixel each.
///
/// As `Vec<bool>` that is 113 MB on a 6144² mono frame against 14 MB packed — enough to decide
/// whether a stack fits its budget, and completely invisible to the correctness tests, which run
/// on frames small enough that either choice is free.
#[test]
fn cosmic_ray_masks_stay_one_bit_per_pixel() {
    for side in [1024usize, 6144] {
        let size = Size2us::new(side, side);
        let pixels = size.pixel_count();
        let packed = new_cr_mask(size).words.len() * size_of::<u64>();

        // Each row pads to 128 bits; 1024 and 6144 are both multiples, so there is no slack here.
        assert_eq!(
            packed,
            pixels / 8,
            "{side}²: {packed} B is not one bit per pixel"
        );
        assert_eq!(
            pixels * size_of::<bool>() / packed,
            8,
            "{side}²: packing should be 8x"
        );
    }

    // The peak is what actually decides whether a stack fits, so pin that rather than the count on
    // its own: 6144² = 37 748 736 px, 4 718 592 B per packed mask, three of them = 14 155 776 B.
    // A fourth concurrent mask, or a revert to `Vec<bool>`, both land here.
    let size = Size2us::new(6144, 6144);
    let peak = CONCURRENT_MASKS * new_cr_mask(size).words.len() * size_of::<u64>();
    assert_eq!(peak, 14_155_776, "peak cosmic-ray mask footprint");
}

/// The mono detector's `f32` scratch is five frame-sized planes, *whatever* `niter` is — it is
/// allocated on the first iteration and reused by the rest.
///
/// This is the side of the detector's footprint that dwarfs the masks above: 720 MB on a 6144²
/// frame against their 14 MB. A plane added back (or a stage that allocates its own again) costs
/// 144 MB a piece and would otherwise show up only as a stack that no longer fits its budget.
#[test]
fn cosmic_ray_float_scratch_is_five_frame_planes() {
    let size = Size2us::new(64, 64);
    let mut data = vec![0.05f32; size.pixel_count()];
    data[size.index_of(Vec2us::new(32, 32))] = 0.95;

    // The spike has to be flagged, or the measurement below would come from a detect-only first
    // pass that never reaches `replace_flagged`'s snapshot buffer.
    let repaired = reject_cosmic_rays(
        &mut cfa_from_plane(
            Buffer2::new(size.width, size.height, data.clone()),
            CfaType::Mono,
        ),
        &CosmicRayConfig::default(),
    );
    assert_eq!(repaired, 1, "the fixture must in-paint something");

    let floats = mono_scratch_floats(&mut data, size, &CosmicRayConfig::default());
    assert_eq!(
        floats,
        MONO_SCRATCH_PLANES * size.pixel_count(),
        "one plane each, exactly — no stage grew a buffer past the frame"
    );

    // 6144² = 37 748 736 px × 4 B = 144 MB a plane, five of them = 720 MB.
    let big = Size2us::new(6144, 6144);
    assert_eq!(
        MONO_SCRATCH_PLANES * big.pixel_count() * size_of::<f32>(),
        754_974_720,
        "mono cosmic-ray float working set"
    );
}
