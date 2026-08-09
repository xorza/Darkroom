//! What the calibration masters hold that is not frame data.
//!
//! The tiered loader's frame budgeting is covered by `combine`'s `mem_budget`; this pins the
//! scratch that budget does not model, and that nothing else would notice growing.

use crate::math::size2us::Size2us;
use crate::stacking::calibration_masters::cosmic_ray::internals::{CONCURRENT_MASKS, new_cr_mask};

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

    // The figure that makes the packing worth three times its face value.
    assert_eq!(CONCURRENT_MASKS, 3);
}
