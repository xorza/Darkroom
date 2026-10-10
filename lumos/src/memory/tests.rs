use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::io::raw::demosaic::bayer::{CfaPattern, rcd};
use crate::io::raw::demosaic::xtrans::markesteijn;
use crate::math::size2us::Size2us;
use crate::memory::chunk_memory_layout::{ChunkMemoryLayout, ChunkRows, MIN_CHUNK_ROWS};
use crate::memory::memory_plan::{MemoryPlan, PerFrameBytes, RunShape};
use crate::memory::*;
use crate::stack_product::quality_planes::QualityPlanes;

const MIB: u64 = 1024 * 1024;
/// The detector's planes these plans are written for. The planner's arithmetic is what they pin;
/// the detector's own count is pinned against the planner in `star_detection::mem_budget`.
const DETECTOR_PLANES: usize = 7;
const GB: u64 = 1024 * MIB;
const FRAME_96MB: usize = 6240 * 4160 * size_of::<f32>();

/// The `System` behind this is a process-wide singleton reused across calls, so the second
/// call must be as good as the first — a stale or half-refreshed instance would show up as a
/// zero or as a value that stops tracking.
#[test]
fn available_memory_reports_a_plausible_figure_on_every_call() {
    let first = available_memory();
    let second = available_memory();

    assert!(first > 0, "no available memory reported");
    assert!(second > 0, "second call reported none");
    // Both are sampled within microseconds of each other on an otherwise-idle test process, so
    // they cannot differ by more than a small fraction without the refresh being broken.
    let (low, high) = (first.min(second), first.max(second));
    assert!(
        high - low < high / 4,
        "consecutive samples disagree wildly: {first} then {second}"
    );
    // A budget derived from it stays inside the machine, which a garbage reading would not.
    assert!(memory_budget(first) < first);
}

#[test]
fn quality_planes_are_charged_per_pixel_not_per_sample() {
    // Coverage and confidence are channel-independent, so an RGB frame carries two of them, not
    // six. Charging per sample would triple the figure and push a masked colour stack to disk
    // for planes it never allocates.
    let mono = ImageDimensions::new((100, 50), 1);
    let rgb = ImageDimensions::new((100, 50), 3);
    assert_eq!(mono.quality_plane_bytes(), 2 * 100 * 50 * 4);
    assert_eq!(rgb.quality_plane_bytes(), mono.quality_plane_bytes());

    // Against the frame's own pixels, which *are* per sample: a masked mono frame is three
    // planes resident and a masked RGB one is five, not six.
    assert_eq!(
        mono.frame_bytes() + mono.quality_plane_bytes(),
        3 * 100 * 50 * 4
    );
    assert_eq!(
        rgb.frame_bytes() + rgb.quality_plane_bytes(),
        5 * 100 * 50 * 4
    );
}

#[test]
fn memory_budget_keeps_one_quarter_as_headroom_without_overflow() {
    assert_eq!(
        memory_budget(8 * 1024 * 1024 * 1024),
        6 * 1024 * 1024 * 1024
    );
    assert_eq!(memory_budget(u64::MAX), 13_835_058_055_282_163_711);
}

/// Each row's fan-out by hand. `FRAME_96MB` is 99.02 MiB, so its transient is 198.05 MiB.
/// - 27 GiB → 20 736 MiB usable; 20 resident frames take 1980.5, leaving 18 755.5 for 94 transients:
///   the 16 workers bind.
/// - 25 GiB → 19 200 MiB usable, less than 200 resident frames' 19 804: no headroom, pinned to 1.
/// - 4 GiB → 3 GiB usable: three 1 GiB transients, one 2 GiB one, and fifteen 96 MB-frame ones of
///   which the 8 workers bind.
/// - 2 GiB → 1.5 GiB usable: one 1 GiB transient. 8 GiB → 6 GiB: six.
/// - Nothing available, or no workers: still one, so the run makes progress.
#[test]
fn load_concurrency_accounts_for_resident_and_transient_memory() {
    let gib = GB as usize;
    let cases = [
        (FRAME_96MB, 2 * FRAME_96MB, 20, 27 * GB, 16, 16),
        (FRAME_96MB, 2 * FRAME_96MB, 200, 25 * GB, 16, 1),
        (gib, gib, 0, 4 * GB, 64, 3),
        (gib, 2 * gib, 0, 4 * GB, 64, 1),
        (FRAME_96MB, 2 * FRAME_96MB, 0, 4 * GB, 8, 8),
        (gib, gib, 0, 2 * GB, 16, 1),
        (gib, gib, 0, 8 * GB, 16, 6),
        (0, 0, 0, 0, 16, 1),
        (FRAME_96MB, 2 * FRAME_96MB, 5, 27 * GB, 0, 1),
    ];

    for (resident, transient, frames, available, workers, expected) in cases {
        assert_eq!(
            load_concurrency(resident, transient, frames, available, workers),
            expected,
            "{resident} B × {frames} resident, {transient} B transient, {available} B, {workers} workers"
        );
    }
}

/// A stack decoded straight into the combine fits when its frames, one decode's statistics scratch
/// and the combine's resident output all fit. Ten 4 MiB frames are 40 MiB, one scratch frame
/// makes the decode peak 44, and two output planes make the combine peak 48: 48 MiB usable is
/// the boundary, and output planes the frames alone leave room for still spill the run.
#[test]
fn a_decoded_stack_charges_its_frames_scratch_and_output() {
    let frame = plane(4);
    let shape =
        |output_planes: usize| RunShape::decoded_stack(10, frame, frame, output_planes * frame);
    let boundary = available_for_usable(48 * MIB);
    assert!(MemoryPlan::plan(shape(2), 8, boundary).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(2), 8, boundary - 2).fits_in_ram);
    assert!(MemoryPlan::plan(shape(0), 8, boundary - 2).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(3), 8, boundary).fits_in_ram);

    // A masked frame's quality planes are resident, the scratch is one plain frame on top: 30 MiB
    // resident, 34 MiB decode peak.
    let masked = RunShape::decoded_stack(10, 3 * MIB as usize, MIB as usize, 0);
    assert_eq!(masked.decode.output_bytes, 3 * MIB as usize);
    assert_eq!(masked.decode.peak_bytes, 4 * MIB as usize);
    assert!(MemoryPlan::plan(masked, 8, available_for_usable(31 * MIB)).fits_in_ram);
    assert!(!MemoryPlan::plan(masked, 8, available_for_usable(31 * MIB) - 2).fits_in_ram);

    // Saturating arithmetic: a frame count no budget can hold never wraps into "fits".
    assert!(
        !MemoryPlan::plan(RunShape::decoded_stack(2, usize::MAX, 1, 0), 8, u64::MAX).fits_in_ram
    );
}

/// The combine's output planes are charged beside the warped frames, not only the decode and
/// warp peaks: at `QualityPlanes::STANDARD` an RGB output is 3 × (image, weight, variance) + coverage
/// = 10 planes and a flag byte a pixel, ¼ plane, which flips a set the warp stage alone would keep
/// in RAM.
#[test]
fn a_warped_run_charges_the_combine_output() {
    let plane_bytes = plane(10);
    let output = QualityPlanes::STANDARD.resident_bytes(ImageDimensions::new(
        ((10 * MIB) as usize / size_of::<f32>(), 1),
        3,
    ));
    assert_eq!(output, 10 * plane_bytes + plane_bytes / 4);
    let shape = |output_bytes| pipeline_shape(plane_bytes, mono(plane_bytes), 5, output_bytes);
    // Mono, five frames, one worker: the decode peaks at 5 frames + its 1P statistics copy and the
    // 7P detector = 13P; the warp at 5 × 3¼P warped + the one worker's 1P source = 17¼P; and the
    // combine at 16¼P + the output's 10¼P = 26½P, which decides. In quarter planes: 106 and 69.
    let quarters = |count: u64| available_for_usable(count * 10 * MIB / 4);
    assert!(MemoryPlan::plan(shape(output), 1, quarters(106)).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(output), 1, quarters(106) - 2).fits_in_ram);
    assert!(MemoryPlan::plan(shape(0), 1, quarters(69)).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(0), 1, quarters(69) - 2).fits_in_ram);
}

/// 30 RGB frames of 24 MP that the caller holds, eight workers: planes of 96 MB, frames of 288 MB.
/// The reading already left the 30 × 288 MB = 8.64 GB of inputs out. A warped frame is 288 + 2 ×
/// 96 + 24 = 504 MB, less the 288 MB source it frees: 30 × 216 MB = 6.48 GB, plus the eight
/// sources in flight, 8 × 288 MB, is the warp's 8.784 GB peak. The decode pass adds the statistics
/// copy and the detector, 288 + 672 = 960 MB, and eight of those fit beside nothing resident. The
/// run stays resident at a budget of exactly 8.784 GB and spills a byte below it. Charged as frames
/// the run decodes, the warp needs 30 × 504 MB + 2.304 GB = 17.424 GB and spills at the same budget.
#[test]
fn held_frames_are_charged_only_what_the_run_adds() {
    const MB: usize = 1_000_000;
    let (plane_bytes, frame_bytes) = (96 * MB, 288 * MB);
    let shape = |held_bytes| RunShape {
        frame_count: 30,
        decode: DemosaicMemory {
            output_bytes: frame_bytes,
            peak_bytes: DECODE_TRANSIENT_FACTOR * frame_bytes,
        },
        held_bytes,
        detection_bytes: DETECTOR_PLANES * plane_bytes,
        warp: Some(PerFrameBytes::new(plane_bytes, frame_bytes)),
        output_bytes: 0,
    };
    assert_eq!(
        PerFrameBytes::new(plane_bytes, frame_bytes).warped,
        504 * MB
    );
    let peak = 8_784 * MB as u64;
    let held = MemoryPlan::plan(shape(frame_bytes), 8, available_for_usable(peak));
    assert_eq!(
        held,
        MemoryPlan {
            fits_in_ram: true,
            decode_concurrency: 8,
            warp_concurrency: 8,
        }
    );
    assert!(!MemoryPlan::plan(shape(frame_bytes), 8, available_for_usable(peak - 1)).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(0), 8, available_for_usable(peak)).fits_in_ram);
    assert!(MemoryPlan::plan(shape(0), 8, available_for_usable(17_424 * MB as u64)).fits_in_ram);
}

/// One pass over ten frames of 10 MiB planes, eight threads: a decode peaks at 7P, the
/// detector holds 7P, and a warped frame is 5¼P. A worker holds 14P, and on the spill tier its 5¼P
/// warp buffers too. Resident needs the 52½P warped set beside one worker, 66½P, where one worker
/// fits; at 164½P all eight do. A byte below 66½P the run spills, and three 19¼P workers fit.
#[test]
fn a_single_pass_holds_the_warped_set_beside_its_workers() {
    let plane_bytes = plane(10);
    let shape = pipeline_shape(plane_bytes, three_channel(plane_bytes, 7), 10, 0);
    let quarters = |count: u64| available_for_usable(count * plane_bytes as u64 / 4);
    let at = |available| MemoryPlan::single_pass(shape, 8, available);
    let concurrency = |plan: MemoryPlan| {
        assert_eq!(plan.decode_concurrency, plan.warp_concurrency);
        (plan.fits_in_ram, plan.decode_concurrency)
    };
    assert_eq!(concurrency(at(quarters(266))), (true, 1));
    assert_eq!(concurrency(at(quarters(658))), (true, 8));
    assert_eq!(concurrency(at(quarters(266) - 2)), (false, 3));
}

/// Rows per chunk by hand: the usable budget over the bytes a row of every input plane costs,
/// after the resident planes, floored at `MIN_CHUNK_ROWS`, and what the floor holds past the
/// budget.
/// - 6000 px × 60 planes (3 channels × 20 frames) × 4 B = 1 440 000 B a row: 6 GiB usable of 8 is
///   4473.9 rows, 768 MiB of 1 GiB is 559.2, 192 MiB of 256 MiB is 139.8.
/// - 6000 px × 20 mono planes = 480 000 B a row: 6 GiB is 13 421.8 rows.
/// - Nothing available: the floor.
/// - 1 MiB available is 786 432 B usable; six resident 100×200 planes take 480 000, and nine input
///   planes cost 3600 B a row, so 85 whole rows fit. A resident flag plane adds 20 000 B, which
///   leaves 79.
/// - Ten resident planes take 800 000 B, past the budget: the floor, holding 64 rows of 3600 B
///   beside them, 800 000 + 230 400 − 786 432 = 243 968 B over. Nothing available holds 64 rows of
///   800 B over.
/// - A 30-row image whose 60 resident planes take 720 000 B fits 18 rows; under the floor it holds
///   its 30 rows, 720 000 + 108 000 − 786 432 = 41 568 B over.
#[test]
fn chunk_rows_match_budget_arithmetic() {
    let layout = |input_planes: usize, resident_planes: usize, flags: usize| ChunkMemoryLayout {
        input_bytes: input_planes * size_of::<f32>(),
        resident_bytes: resident_planes * size_of::<f32>() + flags,
    };
    let fits = |rows| ChunkRows {
        rows,
        overcommit_bytes: 0,
    };
    let floor = |overcommit_bytes| ChunkRows {
        rows: MIN_CHUNK_ROWS,
        overcommit_bytes,
    };
    for (layout, size, available, expected) in [
        (
            layout(60, 0, 0),
            Size2us::new(6000, 100),
            8 * GB,
            fits(4473),
        ),
        (layout(60, 0, 0), Size2us::new(6000, 100), GB, fits(559)),
        (
            layout(60, 0, 0),
            Size2us::new(6000, 100),
            256 * MIB,
            fits(139),
        ),
        (
            layout(20, 0, 0),
            Size2us::new(6000, 100),
            8 * GB,
            fits(13_421),
        ),
        (layout(2, 0, 0), Size2us::new(100, 100), 0, floor(64 * 800)),
        (layout(9, 6, 0), Size2us::new(100, 200), MIB, fits(85)),
        (layout(9, 6, 1), Size2us::new(100, 200), MIB, fits(79)),
        (
            layout(9, 10, 0),
            Size2us::new(100, 200),
            MIB,
            floor(243_968),
        ),
        (layout(9, 60, 0), Size2us::new(100, 30), MIB, floor(41_568)),
        (
            layout(60, 3, 0),
            Size2us::new(0, 100),
            8 * GB,
            fits(MIN_CHUNK_ROWS),
        ),
    ] {
        assert_eq!(
            layout.chunk_rows(size, available),
            expected,
            "{layout:?} over {size:?} at {available} B"
        );
    }
}

fn plane(mib: u64) -> usize {
    (mib * MIB) as usize
}

/// What each demosaic costs for a frame whose planes are `plane_bytes`, from the demosaics' own
/// accounting, on a one-row frame.
fn demosaic(cfa_type: CfaType, plane_bytes: usize) -> DemosaicMemory {
    cfa_type.demosaic_memory(ImageDimensions::new((plane_bytes / size_of::<f32>(), 1), 1))
}

fn mono(plane_bytes: usize) -> DemosaicMemory {
    demosaic(CfaType::Mono, plane_bytes)
}

/// A three-channel decode that peaks at `planes` planes. Fixed, where the demosaics' own peaks
/// carry the pool's tile buffers, which vary with the machine: 7 is the planner under a decode
/// heavier than its output and its statistics, 22 under one far heavier.
fn three_channel(plane_bytes: usize, planes: usize) -> DemosaicMemory {
    DemosaicMemory {
        output_bytes: 3 * plane_bytes,
        peak_bytes: planes * plane_bytes,
    }
}

fn bayer(plane_bytes: usize) -> DemosaicMemory {
    demosaic(CfaType::Bayer(CfaPattern::Rggb), plane_bytes)
}

/// The planes the boundary arithmetic below is written in: output and peak are 1 and 1 planes for
/// mono. Both demosaics' are 3 and 4 (the frame and the output) and the pool's tile buffers.
#[test]
fn demosaic_costs_in_planes() {
    let plane_bytes = plane(10);
    let memory = mono(plane_bytes);
    assert_eq!(memory.output_bytes, plane_bytes);
    assert_eq!(memory.peak_bytes, plane_bytes);
    for (memory, workspace) in [
        (bayer(plane_bytes), rcd::workspace_bytes()),
        (
            demosaic(CfaType::XTrans(XTRANS_PATTERN), plane_bytes),
            markesteijn::workspace_bytes(),
        ),
    ] {
        assert_eq!(memory.output_bytes, 3 * plane_bytes);
        assert_eq!(memory.peak_bytes, 4 * plane_bytes + workspace);
    }
}

/// The smallest availability whose budget is `usable`: the inverse of [`memory_budget`].
fn available_for_usable(usable: u64) -> u64 {
    (usable * 100).div_ceil(MEMORY_PERCENT)
}

/// The shape `calibrate_align_stack` plans `frames` frames with, cosmic rays off: the decode
/// raised to the statistics' copy of every channel beside the frame, the detector's 7 planes
/// beside each decode, and the warp of the demosaiced output.
fn pipeline_shape(
    plane_bytes: usize,
    demosaic: DemosaicMemory,
    frames: usize,
    output_bytes: usize,
) -> RunShape {
    RunShape {
        frame_count: frames,
        decode: demosaic.with_peak_at_least(DECODE_TRANSIENT_FACTOR * demosaic.output_bytes),
        held_bytes: 0,
        detection_bytes: DETECTOR_PLANES * plane_bytes,
        warp: Some(PerFrameBytes::new(plane_bytes, demosaic.output_bytes)),
        output_bytes,
    }
}

/// [`pipeline_shape`] with no output charge: the decode and warp peaks alone, which is what these
/// tests pin.
fn plan(
    plane_bytes: usize,
    demosaic: DemosaicMemory,
    frames: usize,
    threads: usize,
    available: u64,
) -> MemoryPlan {
    MemoryPlan::plan(
        pipeline_shape(plane_bytes, demosaic, frames, 0),
        threads,
        available,
    )
}

/// The per-frame figures every plan below is derived from, in planes. A warped frame is its output,
/// two quality planes and a quarter-plane of flags: 3¼P mono, 5¼P colour. A warp holds the source
/// beside it: 4¼P and 8¼P. In quarter planes: 13 and 17, 21 and 33.
#[test]
fn a_warp_holds_its_source_beside_the_warped_frame() {
    let plane_bytes = plane(10);
    let quarter = plane_bytes / 4;
    for (output, warped, working) in [(1, 13, 17), (3, 21, 33)] {
        assert_eq!(
            PerFrameBytes::new(plane_bytes, output * plane_bytes),
            PerFrameBytes {
                warped: warped * quarter,
                working: working * quarter,
            }
        );
    }
}

/// 100 MiB planes, ten frames of a 22P decode, eight workers, 6 GiB = 61.44P usable. The warped set alone
/// is 52½P, and the decode pass 30P + one 26P transient; it is the eight workers' 3P sources on top
/// of the warped set, 76½P, that force the spill.
#[test]
fn scratch_reserve_streams_a_set_whose_frames_alone_would_fit() {
    let plane_bytes = plane(100);
    let (frames, threads, available) = (10, 8, 8 * GB);
    let demosaic = three_channel(plane_bytes, 22);

    let warped = PerFrameBytes::new(plane_bytes, demosaic.output_bytes).warped;
    assert!((warped * frames) as u64 <= memory_budget(available));
    assert!(!plan(plane_bytes, demosaic, frames, threads, available).fits_in_ram);
}

/// 100 MiB planes, ten frames, eight workers, 61.44P usable.
/// - Mono stays resident: the decode pass is 10P + 8P (its 1P statistics copy and the 7P detector),
///   the warp 32½P + 8 × 1P. Decodes take 8P each from the 51.44P beyond the frames: six. The
///   warp's 1P sources fit all eight workers.
/// - The 7P decode spills on the warp, 52½P + 8 × 3P = 76½P. Spilled, a decode is its 7P peak and the 7P
///   detector, 14P: four fit. A warp is 8¼P: seven.
/// - The 22P decode spills likewise. A decode is its 22P peak and the detector, 29P: two fit.
#[test]
fn fan_out_follows_each_demosaics_peak() {
    let plane_bytes = plane(100);
    for (demosaic, expected) in [
        (
            mono(plane_bytes),
            MemoryPlan {
                fits_in_ram: true,
                decode_concurrency: 6,
                warp_concurrency: 8,
            },
        ),
        (
            three_channel(plane_bytes, 7),
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 4,
                warp_concurrency: 7,
            },
        ),
        (
            three_channel(plane_bytes, 22),
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 2,
                warp_concurrency: 7,
            },
        ),
    ] {
        assert_eq!(plan(plane_bytes, demosaic, 10, 8, 8 * GB), expected);
    }
}

/// 10 MiB planes, five frames of a 22P decode, eight workers, 614.4P usable: the decode pass is 15P + 26P
/// and the warp 26¼P + 5 × 3P, so it fits, and the frame count binds both fan-outs.
#[test]
fn small_set_uses_all_workers_in_ram() {
    let plane_bytes = plane(10);
    assert_eq!(
        plan(plane_bytes, three_channel(plane_bytes, 22), 5, 8, 8 * GB),
        MemoryPlan {
            fits_in_ram: true,
            decode_concurrency: 5,
            warp_concurrency: 5,
        }
    );
}

/// 10 MiB planes, five frames, four workers. Each demosaic's RAM-tier boundary is its larger peak:
/// the 22P decode's pass, 5 × 3P + 26P = 41P, above its warp's 5 × 5¼P + 4 × 3P = 38¼P; the 7P one's
/// warp, 38¼P, above its 26P decode pass; mono's warp, 5 × 3¼P + 4 × 1P = 20¼P, above its 13P
/// decode pass. In quarter planes: 164, 153 and 81.
#[test]
fn ram_tier_respects_algorithm_specific_concurrency_boundaries() {
    let plane_bytes = plane(10);
    let (frames, threads) = (5, 4);
    for (demosaic, boundary_quarters) in [
        (three_channel(plane_bytes, 22), 164),
        (three_channel(plane_bytes, 7), 153),
        (mono(plane_bytes), 81),
    ] {
        let boundary = available_for_usable(boundary_quarters * 10 * MIB / 4);
        assert!(plan(plane_bytes, demosaic, frames, threads, boundary).fits_in_ram);
        assert!(!plan(plane_bytes, demosaic, frames, threads, boundary - 2).fits_in_ram);
    }

    // At 41P all three fit, and their decode transients buy different fan-outs from what the
    // resident outputs leave: the 22P decode's 26P one of 26P, the 7P one's 11P two, mono's 8P four of
    // 36P.
    let at = available_for_usable(410 * MIB);
    for (demosaic, decode_concurrency) in [
        (three_channel(plane_bytes, 22), 1),
        (three_channel(plane_bytes, 7), 2),
        (mono(plane_bytes), 4),
    ] {
        assert_eq!(
            plan(plane_bytes, demosaic, frames, threads, at).decode_concurrency,
            decode_concurrency
        );
    }

    // Headroom scales the 22P decode's fan-out: 67P usable leaves 52P, two transients; 93P leaves 78P,
    // three.
    for (usable_planes, decode_concurrency) in [(67, 2), (93, 3)] {
        assert_eq!(
            plan(
                plane_bytes,
                three_channel(plane_bytes, 22),
                frames,
                threads,
                available_for_usable(usable_planes * 10 * MIB),
            )
            .decode_concurrency,
            decode_concurrency
        );
    }
}

/// For every frame size, count, worker count and budget, the planned fan-out keeps each stage's
/// projected peak — the resident set plus `concurrency ×` what one in-flight frame adds to it —
/// within the usable budget, unless not even one frame fits and the fan-out is pinned to 1. Swept
/// for the pipeline's shape over each demosaic and the fixed decodes, and for stacks decoded
/// straight into the combine.
#[test]
fn planned_concurrency_never_overshoots_its_tier_budget() {
    for &plane_mib in &[16u64, 64, 100, 400] {
        let plane_bytes = plane(plane_mib);
        let decoded = |frames| RunShape::decoded_stack(frames, plane_bytes, plane_bytes, 0);
        for &frames in &[4usize, 12, 30, 60] {
            let shapes = [
                mono(plane_bytes),
                bayer(plane_bytes),
                demosaic(CfaType::XTrans(XTRANS_PATTERN), plane_bytes),
                three_channel(plane_bytes, 7),
                three_channel(plane_bytes, 22),
            ]
            .map(|demosaic| pipeline_shape(plane_bytes, demosaic, frames, 0))
            .into_iter()
            .chain([decoded(frames)]);
            for shape in shapes {
                let (decode, detection) = (shape.decode, shape.detection_bytes as u64);
                let (output, peak) = (decode.output_bytes as u64, decode.peak_bytes as u64);
                let (warped, working) = shape.warp.map_or((output, 0), |per_frame| {
                    (per_frame.warped as u64, per_frame.working as u64)
                });
                for &threads in &[1usize, 8, 32] {
                    for &budget_gib in &[1u64, 2, 4, 8, 16] {
                        let available = budget_gib * GB;
                        let plan = MemoryPlan::plan(shape, threads, available);
                        let usable = memory_budget(available);
                        let worker_cap = frames.min(threads.max(1));
                        let n = frames as u64;

                        assert!(plan.decode_concurrency <= worker_cap);
                        assert!(plan.warp_concurrency <= worker_cap);
                        assert!(plan.decode_concurrency >= 1 && plan.warp_concurrency >= 1);

                        let (decode_peak, warp_peak) = if plan.fits_in_ram {
                            (
                                n * output
                                    + (peak - output + detection) * plan.decode_concurrency as u64,
                                n * warped
                                    + working.saturating_sub(warped) * plan.warp_concurrency as u64,
                            )
                        } else {
                            (
                                (peak + detection) * plan.decode_concurrency as u64,
                                working * plan.warp_concurrency as u64,
                            )
                        };
                        assert!(
                            decode_peak <= usable || plan.decode_concurrency == 1,
                            "decode peak {} MiB exceeds {} MiB usable",
                            decode_peak / MIB,
                            usable / MIB
                        );
                        assert!(
                            warp_peak <= usable || plan.warp_concurrency == 1,
                            "warp peak {} MiB exceeds {} MiB usable",
                            warp_peak / MIB,
                            usable / MIB
                        );
                    }
                }
            }
        }
    }
}

/// 100 MiB planes, twenty frames of a 22P decode, sixteen workers.
/// - 2 GiB → 15.36P usable: spilled, under one 29P decode and two 8¼P warps; both pinned or bound
///   to 1.
/// - 16 GiB → 122.88P: the warp, 105P + 16 × 3P, still spills; four 29P decodes and fourteen
///   8¼P warps fit.
/// - 2⁵⁰ B: everything fits and the workers bind.
#[test]
fn budget_flips_the_tier_and_scales_streaming_fanout() {
    let plane_bytes = plane(100);
    let demosaic = three_channel(plane_bytes, 22);
    let (frames, threads) = (20, 16);
    for (available, expected) in [
        (
            2 * GB,
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 1,
                warp_concurrency: 1,
            },
        ),
        (
            16 * GB,
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 4,
                warp_concurrency: 14,
            },
        ),
        (
            1 << 50,
            MemoryPlan {
                fits_in_ram: true,
                decode_concurrency: 16,
                warp_concurrency: 16,
            },
        ),
    ] {
        assert_eq!(
            plan(plane_bytes, demosaic, frames, threads, available),
            expected
        );
    }
}
