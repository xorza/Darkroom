//! What the calibration masters hold that is not frame data.
//!
//! Pins the cosmic-ray scratch the pipeline's decode budget charges against what the detectors
//! allocate; that a run holds no more than its budget is the `memory_budget` integration test's.

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::calibration::cosmic_ray;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::masks::CONCURRENT_MASKS;
use crate::calibration::cosmic_ray::masks::internals::new_cr_mask;
use crate::calibration::cosmic_ray::mono::MONO_SCRATCH_PLANES;
use crate::calibration::cosmic_ray::mono::internals::mono_scratch_floats;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::internals::cfa::cfa_from_plane;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

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
        let packed = BitBuffer2::heap_bytes(new_cr_mask(size).size);

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
    let peak = CONCURRENT_MASKS * BitBuffer2::heap_bytes(new_cr_mask(size).size);
    assert_eq!(peak, 14_155_776, "peak cosmic-ray mask footprint");
}

/// The mono detector's `f32` scratch is five frame-sized planes, *whatever* `max_iterations` is —
/// it is allocated on the first iteration and reused by the rest.
///
/// This is the side of the detector's footprint that dwarfs the masks above: 720 MB on a 6144²
/// frame against their 14 MB. A plane added back (or a stage that allocates its own again) costs
/// 144 MB a piece and would otherwise show up only as a stack that no longer fits its budget.
#[test]
fn cosmic_ray_float_scratch_is_five_frame_planes() {
    let size = Size2us::new(64, 64);
    let mut data = Buffer2::new_filled(size.width, size.height, 0.05f32);
    data[size.idx_of(Vec2us::new(32, 32))] = 0.95;

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

    // 6144² = 37 748 736 px × 4 B = 144 MB a plane, five of them = 720 MB.
    let big = Size2us::new(6144, 6144);
    assert_eq!(
        MONO_SCRATCH_PLANES * big.pixel_count() * size_of::<f32>(),
        754_974_720,
        "mono cosmic-ray float working set"
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
    let bayer = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    for (cfa, side, expected) in [
        (CfaType::Mono, 64, 81_920 + 3_072 + 2_048),
        (bayer, 64, 4_096 + 20_480 + 1_536 + 1_024 + 1_024 + 2_048),
        (CfaType::Mono, 2, 3 * 2 * 16),
        (bayer, 4, 3 * 4 * 16),
    ] {
        assert_eq!(
            cosmic_ray::heap_bytes(&cfa, Size2us::new(side, side)),
            Ok(expected),
            "{cfa:?} {side}²"
        );
    }
}
