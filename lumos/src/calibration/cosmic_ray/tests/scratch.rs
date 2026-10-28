//! The cosmic-ray scratch the pipeline's decode budget charges, against what the detectors
//! allocate; that a run holds no more than its budget is the `memory_budget` integration test's.

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::calibration::cosmic_ray;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::masks::internals::new_cr_mask;
use crate::calibration::cosmic_ray::mono::MONO_SCRATCH_PLANES;
use crate::calibration::cosmic_ray::mono::internals::mono_scratch_floats;
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::internals::cfa::cfa_from_plane;
use common::USize2;
use glam::USizeVec2;

/// Cosmic-ray detection holds three full-frame masks at its peak, one bit per pixel each.
///
/// As `Vec<bool>` that is 113 MB on a 6144² mono frame against 14 MB packed — enough to decide
/// whether a stack fits its budget, and completely invisible to the correctness tests, which run
/// on frames small enough that either choice is free.
#[test]
fn cosmic_ray_masks_stay_one_bit_per_pixel() {
    for side in [1024usize, 6144] {
        let size = USize2::new(side, side);
        let pixels = size.pixel_count();
        let packed = BitBuffer2::heap_bytes(new_cr_mask(size).size());

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
}

/// The mono detector's `f32` scratch is five frame-sized planes, *whatever* `max_iterations` is —
/// it is allocated on the first iteration and reused by the rest.
///
/// This is the side of the detector's footprint that dwarfs the masks above: 720 MB on a 6144²
/// frame against their 14 MB. A plane added back (or a stage that allocates its own again) costs
/// 144 MB a piece and would otherwise show up only as a stack that no longer fits its budget.
#[test]
fn cosmic_ray_float_scratch_is_five_frame_planes() {
    let size = USize2::new(64, 64);
    let mut data = Buffer2::new_filled(size, 0.05f32);
    data[size.idx_of(USizeVec2::new(32, 32))] = 0.95;

    // The spike has to be flagged, or the measurement below would come from a detect-only first
    // pass that never reaches `PixelRepair::apply`'s snapshot buffer.
    let repaired = cosmic_ray::reject(
        &mut cfa_from_plane(data.clone(), CfaType::Mono),
        &CosmicRayConfig::default(),
    )
    .unwrap();
    assert_eq!(repaired, 1, "the fixture must in-paint something");

    let floats = mono_scratch_floats(&mut data, &CosmicRayConfig::default());
    assert_eq!(
        floats,
        MONO_SCRATCH_PLANES * size.pixel_count(),
        "one plane each, exactly — no stage grew a buffer past the frame"
    );
}

/// What the decode budget charges for a cosmic-ray pass, from the planes above and the masks. A
/// mask row pads to 128 bits, two words, so a 64×64 mask is 1024 B. Mono: 5 planes of 4096 px =
/// 81 920 B, the detector's three masks, 3072 B, and the pass's two of kept-out pixels, 2048 B.
/// Bayer runs on its 32×32 phases: the phase plane, 4096 B, and the mono scratch over it, 20 480 B
/// plus three 512 B masks; two more 512 B masks for the phase's kept-out pixels; the 1024 B mask
/// of the mosaic's repairs; and the pass's two frame masks. A frame too small to scan holds the
/// three masks building the kept-out pixels takes, more than the detector's empty mask of repairs
/// and the pass's two: 2 rows of 16 B each at 2×2, 4 at 4×4.
#[test]
fn cosmic_ray_heap_bytes_count_planes_and_masks() {
    let bayer = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    for (cfa, side, expected) in [
        (CfaType::Mono, 64, 81_920 + 3_072 + 2_048),
        (bayer, 64, 4_096 + 20_480 + 1_536 + 1_024 + 1_024 + 2_048),
        (CfaType::Mono, 2, 3 * 2 * 16),
        (bayer, 4, 3 * 4 * 16),
    ] {
        assert_eq!(
            cosmic_ray::heap_bytes(&cfa, USize2::new(side, side)),
            Ok(expected),
            "{cfa:?} {side}²"
        );
    }
}
