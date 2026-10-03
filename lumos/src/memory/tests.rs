use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::image::cfa::CfaType;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::memory::*;
use crate::stack_product::quality_planes::QualityPlanes;

const MIB: u64 = 1024 * 1024;
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
    assert_eq!(quality_plane_bytes(mono), 2 * 100 * 50 * 4);
    assert_eq!(quality_plane_bytes(rgb), quality_plane_bytes(mono));

    // Against the frame's own pixels, which *are* per sample: a masked mono frame is three
    // planes resident and a masked RGB one is five, not six.
    assert_eq!(
        frame_bytes(mono) + quality_plane_bytes(mono),
        3 * 100 * 50 * 4
    );
    assert_eq!(
        frame_bytes(rgb) + quality_plane_bytes(rgb),
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
/// warp peaks: at `QualityPlanes::ALL` an RGB output is 3 × (image, weight, variance) + coverage
/// = 10 planes, which flips a set the warp stage alone would keep in RAM.
#[test]
fn a_warped_run_charges_the_combine_output() {
    let plane_bytes = plane(10);
    let output = QualityPlanes::ALL.resident_bytes(ImageDimensions::new(
        ((10 * MIB) as usize / size_of::<f32>(), 1),
        3,
    ));
    assert_eq!(output, 10 * plane_bytes);
    let shape = |output_bytes| pipeline_shape(plane_bytes, mono(plane_bytes), 5, output_bytes);
    // Mono, five frames, one worker: the decode peaks at 5 frames + its 1P statistics copy and the
    // 7P detector = 13P; the warp at 5 × 3P warped + the one worker's 1P source = 16P; and the
    // combine at 15P + the output's 10P = 25P, which decides.
    assert!(MemoryPlan::plan(shape(output), 1, available_for_usable(25 * 10 * MIB)).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(output), 1, available_for_usable(24 * 10 * MIB)).fits_in_ram);
    assert!(MemoryPlan::plan(shape(0), 1, available_for_usable(16 * 10 * MIB)).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(0), 1, available_for_usable(16 * 10 * MIB) - 2).fits_in_ram);
}

/// Rows per chunk by hand: the usable budget over the bytes a row of every input plane costs,
/// after the resident planes, floored at `MIN_CHUNK_ROWS`.
/// - 6000 px × 60 planes (3 channels × 20 frames) × 4 B = 1 440 000 B a row: 6 GiB usable of 8 is
///   4473.9 rows, 768 MiB of 1 GiB is 559.2, 192 MiB of 256 MiB is 139.8.
/// - 6000 px × 20 mono planes = 480 000 B a row: 6 GiB is 13 421.8 rows.
/// - Nothing available: the floor.
/// - 1 MiB available is 786 432 B usable; six resident 100×200 planes take 480 000, and nine input
///   planes cost 3600 B a row, so 85 whole rows fit. Ten resident planes leave nothing: the floor.
#[test]
fn optimal_chunk_rows_matches_budget_arithmetic() {
    let layout = |input_planes, resident_planes| ChunkMemoryLayout {
        input_planes,
        resident_planes,
    };
    for (layout, size, available, expected) in [
        (layout(60, 0), Size2us::new(6000, 100), 8 * GB, 4473),
        (layout(60, 0), Size2us::new(6000, 100), GB, 559),
        (layout(60, 0), Size2us::new(6000, 100), 256 * MIB, 139),
        (layout(20, 0), Size2us::new(6000, 100), 8 * GB, 13_421),
        (layout(2, 0), Size2us::new(100, 100), 0, MIN_CHUNK_ROWS),
        (layout(9, 6), Size2us::new(100, 200), MIB, 85),
        (layout(9, 10), Size2us::new(100, 200), MIB, MIN_CHUNK_ROWS),
        (layout(60, 3), Size2us::new(0, 100), 8 * GB, MIN_CHUNK_ROWS),
    ] {
        assert_eq!(
            layout.optimal_chunk_rows(size, available),
            expected,
            "{layout:?} over {size:?} at {available} B"
        );
    }
}

fn plane(mib: u64) -> usize {
    (mib * MIB) as usize
}

/// What each demosaic costs for a frame whose planes are `plane_bytes`, from the demosaics' own
/// accounting: a one-row frame, so RCD's half-width planes are exactly half of it.
fn demosaic(cfa_type: CfaType, plane_bytes: usize) -> DemosaicMemory {
    let width = plane_bytes / size_of::<f32>();
    assert!(
        width.is_multiple_of(2),
        "an even width keeps RCD's half planes exact"
    );
    cfa_type.demosaic_memory(ImageDimensions::new((width, 1), 1))
}

fn mono(plane_bytes: usize) -> DemosaicMemory {
    demosaic(CfaType::Mono, plane_bytes)
}

fn bayer(plane_bytes: usize) -> DemosaicMemory {
    demosaic(CfaType::Bayer(CfaPattern::Rggb), plane_bytes)
}

fn xtrans(plane_bytes: usize) -> DemosaicMemory {
    demosaic(CfaType::XTrans(XTRANS_PATTERN), plane_bytes)
}

/// The planes the boundary arithmetic below is written in: output and peak are 1 and 1 planes for
/// mono, 3 and 7 for RCD (six full planes and two half ones in its directional pass, four and the
/// output in its last), and 3 and 22 for Markesteijn (the frame, its 18-word arena, the output).
#[test]
fn demosaic_costs_in_planes() {
    let plane_bytes = plane(10);
    for (memory, output, peak) in [
        (mono(plane_bytes), 1, 1),
        (bayer(plane_bytes), 3, 7),
        (xtrans(plane_bytes), 3, 22),
    ] {
        assert_eq!(memory.output_bytes, output * plane_bytes);
        assert_eq!(memory.peak_bytes, peak * plane_bytes);
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
        detection_bytes: DETECTION_WORKING_PLANES * plane_bytes,
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

/// The per-frame figures every plan below is derived from, in planes. A warped frame is its output
/// and two quality planes: 3P mono, 5P colour. A warp holds the source beside it: 4P and 8P.
#[test]
fn a_warp_holds_its_source_beside_the_warped_frame() {
    let plane_bytes = plane(10);
    for (output, warped, working) in [(1, 3, 4), (3, 5, 8)] {
        assert_eq!(
            PerFrameBytes::new(plane_bytes, output * plane_bytes),
            PerFrameBytes {
                warped: warped * plane_bytes,
                working: working * plane_bytes,
            }
        );
    }
}

/// 100 MiB planes, ten X-Trans frames, eight workers, 6 GiB = 61.44P usable. The warped set alone
/// is 50P, and the decode pass 30P + one 26P transient; it is the eight workers' 3P sources on top
/// of the warped set, 74P, that force the spill.
#[test]
fn scratch_reserve_streams_a_set_whose_frames_alone_would_fit() {
    let plane_bytes = plane(100);
    let (frames, threads, available) = (10, 8, 8 * GB);
    let demosaic = xtrans(plane_bytes);

    let warped = PerFrameBytes::new(plane_bytes, demosaic.output_bytes).warped;
    assert!((warped * frames) as u64 <= memory_budget(available));
    assert!(!plan(plane_bytes, demosaic, frames, threads, available).fits_in_ram);
}

/// 100 MiB planes, ten frames, eight workers, 61.44P usable.
/// - Mono stays resident: the decode pass is 10P + 8P (its 1P statistics copy and the 7P detector),
///   the warp 30P + 8 × 1P. Decodes take 8P each from the 51.44P beyond the frames: six. The warp's
///   1P sources fit all eight workers.
/// - Bayer spills on the warp, 50P + 8 × 3P = 74P. Spilled, a decode is its 7P peak and the 7P
///   detector, 14P: four fit. A warp is 8P: seven.
/// - X-Trans spills likewise. A decode is its 22P peak and the detector, 29P: two fit.
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
            bayer(plane_bytes),
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 4,
                warp_concurrency: 7,
            },
        ),
        (
            xtrans(plane_bytes),
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

/// 10 MiB planes, five X-Trans frames, eight workers, 614.4P usable: the decode pass is 15P + 26P
/// and the warp 25P + 5 × 3P, so it fits, and the frame count binds both fan-outs.
#[test]
fn small_set_uses_all_workers_in_ram() {
    let plane_bytes = plane(10);
    assert_eq!(
        plan(plane_bytes, xtrans(plane_bytes), 5, 8, 8 * GB),
        MemoryPlan {
            fits_in_ram: true,
            decode_concurrency: 5,
            warp_concurrency: 5,
        }
    );
}

/// 10 MiB planes, five frames, four workers. Each demosaic's RAM-tier boundary is its larger peak:
/// X-Trans's decode pass, 5 × 3P + 26P = 41P; Bayer's warp, 5 × 5P + 4 × 3P = 37P, above its 26P
/// decode pass; mono's warp, 5 × 3P + 4 × 1P = 19P, above its 13P decode pass.
#[test]
fn ram_tier_respects_algorithm_specific_concurrency_boundaries() {
    let plane_bytes = plane(10);
    let (frames, threads) = (5, 4);
    for (demosaic, boundary_planes) in [
        (xtrans(plane_bytes), 41),
        (bayer(plane_bytes), 37),
        (mono(plane_bytes), 19),
    ] {
        let boundary = available_for_usable(boundary_planes * 10 * MIB);
        assert!(plan(plane_bytes, demosaic, frames, threads, boundary).fits_in_ram);
        assert!(!plan(plane_bytes, demosaic, frames, threads, boundary - 2).fits_in_ram);
    }

    // At 41P all three fit, and their decode transients buy different fan-outs from what the
    // resident outputs leave: X-Trans's 26P one of 26P, Bayer's 11P two, mono's 8P four of 36P.
    let at = available_for_usable(410 * MIB);
    for (demosaic, decode_concurrency) in [
        (xtrans(plane_bytes), 1),
        (bayer(plane_bytes), 2),
        (mono(plane_bytes), 4),
    ] {
        assert_eq!(
            plan(plane_bytes, demosaic, frames, threads, at).decode_concurrency,
            decode_concurrency
        );
    }

    // Headroom scales the X-Trans fan-out: 67P usable leaves 52P, two transients; 93P leaves 78P,
    // three.
    for (usable_planes, decode_concurrency) in [(67, 2), (93, 3)] {
        assert_eq!(
            plan(
                plane_bytes,
                xtrans(plane_bytes),
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
/// for the pipeline's shape over each demosaic and for stacks decoded straight into the combine.
#[test]
fn planned_concurrency_never_overshoots_its_tier_budget() {
    for &plane_mib in &[16u64, 64, 100, 400] {
        let plane_bytes = plane(plane_mib);
        let decoded = |frames| RunShape::decoded_stack(frames, plane_bytes, plane_bytes, 0);
        for &frames in &[4usize, 12, 30, 60] {
            let shapes = [mono(plane_bytes), bayer(plane_bytes), xtrans(plane_bytes)]
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

/// 100 MiB planes, twenty X-Trans frames, sixteen workers.
/// - 2 GiB → 15.36P usable: spilled, under one 29P decode and two 8P warps; both pinned or bound
///   to 1.
/// - 16 GiB → 122.88P: the warp, 100P + 16 × 3P, still spills; four 29P decodes and fifteen 8P
///   warps fit.
/// - 2⁵⁰ B: everything fits and the workers bind.
#[test]
fn budget_flips_the_tier_and_scales_streaming_fanout() {
    let plane_bytes = plane(100);
    let demosaic = xtrans(plane_bytes);
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
                warp_concurrency: 15,
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
