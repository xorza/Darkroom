use crate::io::image::cfa::CfaType;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::memory::*;
use crate::stacking::stack_product::quality_planes::QualityPlanes;
use crate::testing::cfa::XTRANS_PATTERN;

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

#[test]
fn load_concurrency_accounts_for_resident_and_transient_memory() {
    let cases = [
        (FRAME_96MB, 2 * FRAME_96MB, 20, 27 * GB, 16, 16),
        (FRAME_96MB, 2 * FRAME_96MB, 200, 25 * GB, 16, 1),
        (GB as usize, GB as usize, 0, 4 * GB, 64, 3),
        (GB as usize, 2 * GB as usize, 0, 4 * GB, 64, 1),
        (FRAME_96MB, 2 * FRAME_96MB, 0, 4 * GB, 8, 8),
        (GB as usize, GB as usize, 0, 2 * GB, 16, 1),
        (GB as usize, GB as usize, 0, 8 * GB, 16, 6),
        (0, 0, 0, 0, 16, 1),
        (FRAME_96MB, 2 * FRAME_96MB, 5, 27 * GB, 0, 1),
    ];

    for (resident, transient, frames, available, workers, expected) in cases {
        assert_eq!(
            load_concurrency(resident, transient, frames, available, workers),
            expected
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
    let shape = |output_bytes| RunShape {
        frame_count: 5,
        decode: mono(plane_bytes),
        warp: Some(PerFrameBytes::new(plane_bytes, mono(plane_bytes))),
        output_bytes,
    };
    // Mono: warped 3P each, 15P resident; one worker's working set max(1 + 3, 8) = 8P makes the
    // warp peak 23P, and the output's 10P makes the combine peak 25P, which decides.
    assert!(MemoryPlan::plan(shape(output), 1, available_for_usable(25 * 10 * MIB)).fits_in_ram);
    assert!(!MemoryPlan::plan(shape(output), 1, available_for_usable(24 * 10 * MIB)).fits_in_ram);
    assert!(MemoryPlan::plan(shape(0), 1, available_for_usable(24 * 10 * MIB)).fits_in_ram);
}

#[test]
fn optimal_chunk_rows_matches_budget_arithmetic() {
    let cases = [
        (6000, 3, 20, 8 * GB),
        (1000, 3, 5, 4 * GB),
        (8000, 3, 100, 16 * GB),
        (6000, 3, 20, GB),
        (6000, 3, 20, 256 * 1024 * 1024),
        (6000, 1, 20, 8 * GB),
        (100, 1, 2, 0),
    ];

    for (width, channels, frames, available) in cases {
        let input_planes = channels * frames;
        let bytes_per_row = (width * input_planes * size_of::<f32>()) as u64;
        let usable = memory_budget(available);
        let expected = (usable / bytes_per_row).max(MIN_CHUNK_ROWS as u64) as usize;
        assert_eq!(
            ChunkMemoryLayout {
                input_planes,
                resident_planes: 0,
            }
            .optimal_chunk_rows(Size2us::new(width, 100), available),
            expected
        );
    }

    // 1 MiB available → 786,432 usable bytes. Six resident 100×200 f32 planes consume 480,000
    // bytes; nine active input planes consume 3,600 bytes/row, leaving exactly 85 whole rows.
    assert_eq!(
        ChunkMemoryLayout {
            input_planes: 9,
            resident_planes: 6,
        }
        .optimal_chunk_rows(Size2us::new(100, 200), 1024 * 1024),
        85
    );
    assert_eq!(
        ChunkMemoryLayout {
            input_planes: 60,
            resident_planes: 3,
        }
        .optimal_chunk_rows(Size2us::new(0, 100), 8 * GB),
        MIN_CHUNK_ROWS
    );
    assert_eq!(
        ChunkMemoryLayout {
            input_planes: 9,
            resident_planes: 10,
        }
        .optimal_chunk_rows(Size2us::new(100, 200), 1024 * 1024),
        MIN_CHUNK_ROWS
    );
    assert_eq!(memory_budget(8 * GB), 6 * GB);
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

/// A warping run of `frames` frames whose decode is `demosaic`, with no output charge: the decode and
/// warp peaks alone, which is what these tests pin.
fn plan(
    plane_bytes: usize,
    demosaic: DemosaicMemory,
    frames: usize,
    threads: usize,
    available: u64,
) -> MemoryPlan {
    MemoryPlan::plan(
        RunShape {
            frame_count: frames,
            decode: demosaic,
            warp: Some(PerFrameBytes::new(plane_bytes, demosaic)),
            output_bytes: 0,
        },
        threads,
        available,
    )
}

#[test]
fn scratch_reserve_streams_a_set_whose_frames_alone_would_fit() {
    let plane_bytes = plane(100);
    let (frames, threads, available) = (10, 8, 8 * GB);
    let demosaic = xtrans(plane_bytes);

    // The warped set alone fits; it is the per-worker scratch on top that forces the spill.
    let warped = PerFrameBytes::new(plane_bytes, demosaic).warped;
    assert!((warped * frames) as u64 <= memory_budget(available));
    assert!(!plan(plane_bytes, demosaic, frames, threads, available).fits_in_ram);
}

#[test]
fn streaming_concurrency_uses_the_selected_demosaic_peak() {
    let plane_bytes = plane(100);
    let expected = [
        (
            mono(plane_bytes),
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 7,
                warp_concurrency: 7,
            },
        ),
        (
            bayer(plane_bytes),
            MemoryPlan {
                fits_in_ram: false,
                decode_concurrency: 7,
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
    ];

    for (demosaic, expected) in expected {
        assert_eq!(plan(plane_bytes, demosaic, 10, 8, 8 * GB), expected);
    }
}

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

#[test]
fn ram_tier_respects_algorithm_specific_concurrency_boundaries() {
    let plane_bytes = plane(10);
    let (frames, threads) = (5, 4);

    // 570 MiB usable is exactly the RAM-tier boundary for the two three-channel demosaics:
    // 5 warped planes × 5 frames + 8 working planes × 4 workers = 57 planes.
    let boundary = available_for_usable(570 * MIB);

    // All three fit there, but the demosaic transients buy different decode fan-outs from the
    // 420 MiB left beyond the 3P×5 resident outputs: X-Trans's 19P admits two workers where
    // Bayer's 4P and mono's nothing admit all four.
    assert_eq!(
        plan(plane_bytes, xtrans(plane_bytes), frames, threads, boundary),
        MemoryPlan {
            fits_in_ram: true,
            decode_concurrency: 2,
            warp_concurrency: 4,
        }
    );
    for demosaic in [mono(plane_bytes), bayer(plane_bytes)] {
        let plan = plan(plane_bytes, demosaic, frames, threads, boundary);
        assert_eq!(plan.decode_concurrency, 4);
        assert!(plan.fits_in_ram);
    }

    // A MiB under the boundary and the three-channel pair spills; mono's 47 planes still fit.
    let under = available_for_usable(569 * MIB);
    for demosaic in [bayer(plane_bytes), xtrans(plane_bytes)] {
        assert!(!plan(plane_bytes, demosaic, frames, threads, under).fits_in_ram);
    }
    assert!(plan(plane_bytes, mono(plane_bytes), frames, threads, under).fits_in_ram);

    // Headroom scales the X-Trans fan-out: 760 usable less 150 resident is 610 MiB, three 19P
    // transients' worth.
    assert_eq!(
        plan(
            plane_bytes,
            xtrans(plane_bytes),
            frames,
            threads,
            available_for_usable(760 * MIB),
        )
        .decode_concurrency,
        3
    );
}

/// For every frame size, count, worker count and budget, the planned fan-out keeps each stage's
/// projected peak — the resident set plus `concurrency ×` one in-flight frame — within the usable
/// budget, unless not even one frame fits and the fan-out is pinned to 1. Swept for warping runs
/// of each demosaic and for stacks decoded straight into the combine, whose per-decode transient
/// is the statistics scratch beside the frame.
#[test]
fn planned_concurrency_never_overshoots_its_tier_budget() {
    for &plane_mib in &[16u64, 64, 100, 400] {
        let plane_bytes = plane(plane_mib);
        let decoded = |frames| RunShape::decoded_stack(frames, plane_bytes, plane_bytes, 0);
        for &frames in &[4usize, 12, 30, 60] {
            let shapes = [mono(plane_bytes), bayer(plane_bytes), xtrans(plane_bytes)]
                .map(|demosaic| RunShape {
                    frame_count: frames,
                    decode: demosaic,
                    warp: Some(PerFrameBytes::new(plane_bytes, demosaic)),
                    output_bytes: 0,
                })
                .into_iter()
                .chain([decoded(frames)]);
            for shape in shapes {
                let decode = shape.decode;
                let (warped, working) = shape.warp.map_or((decode.output_bytes, 0), |per_frame| {
                    (per_frame.warped, per_frame.working)
                });
                for &threads in &[1usize, 8, 32] {
                    for &budget_gib in &[1u64, 2, 4, 8, 16] {
                        let available = budget_gib * GB;
                        let plan = MemoryPlan::plan(shape, threads, available);
                        let usable = memory_budget(available);
                        let worker_cap = frames.min(threads.max(1));

                        assert!(plan.decode_concurrency <= worker_cap);
                        assert!(plan.warp_concurrency <= worker_cap);
                        assert!(plan.decode_concurrency >= 1 && plan.warp_concurrency >= 1);

                        let decode_peak = if plan.fits_in_ram {
                            (decode.output_bytes as u64).saturating_mul(frames as u64)
                                + (decode.peak_bytes.saturating_sub(decode.output_bytes) as u64)
                                    .saturating_mul(plan.decode_concurrency as u64)
                        } else {
                            (decode.peak_bytes.max(working) as u64)
                                .saturating_mul(plan.decode_concurrency as u64)
                        };
                        let warp_peak = if plan.fits_in_ram {
                            (warped * frames) as u64 + working as u64 * plan.warp_concurrency as u64
                        } else {
                            working as u64 * plan.warp_concurrency as u64
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

#[test]
fn budget_flips_the_tier_and_scales_streaming_fanout() {
    let plane_bytes = plane(100);
    let demosaic = xtrans(plane_bytes);
    let (frames, threads) = (20, 16);

    let tight = plan(plane_bytes, demosaic, frames, threads, 2 * GB);
    let roomy_streaming = plan(plane_bytes, demosaic, frames, threads, 16 * GB);
    let ample = plan(plane_bytes, demosaic, frames, threads, 1 << 50);

    assert!(!tight.fits_in_ram);
    assert!(!roomy_streaming.fits_in_ram);
    assert!(ample.fits_in_ram);
    assert!(roomy_streaming.decode_concurrency > tight.decode_concurrency);
    assert!(roomy_streaming.warp_concurrency > tight.warp_concurrency);
}
