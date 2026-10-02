use crate::memory::*;

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

#[test]
fn fits_in_memory_honors_budget_boundary_channels_and_overflow() {
    let bytes_per_image = 1000 * 1000 * size_of::<f32>();
    let frame_count = 10;
    let bytes_needed = (bytes_per_image * frame_count) as u64;
    let available_at_boundary = (bytes_needed * 100).div_ceil(75);

    assert!(fits_in_memory(
        bytes_per_image,
        frame_count,
        available_at_boundary
    ));
    assert!(!fits_in_memory(
        bytes_per_image,
        frame_count,
        available_at_boundary - 2
    ));
    assert!(fits_in_memory(6000 * 4000 * 4, 20, 4 * GB));
    assert!(!fits_in_memory(6000 * 4000 * 3 * 4, 20, 4 * GB));
    assert!(!fits_in_memory(usize::MAX, 2, u64::MAX));
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
        let usable = (u128::from(available) * 75 / 100) as u64;
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

fn memory(plane_bytes: usize, output_planes: usize, peak_planes: usize) -> DemosaicMemory {
    DemosaicMemory {
        output_bytes: output_planes * plane_bytes,
        peak_bytes: peak_planes * plane_bytes,
    }
}

fn mono(plane_bytes: usize) -> DemosaicMemory {
    memory(plane_bytes, 1, 1)
}

fn bayer(plane_bytes: usize) -> DemosaicMemory {
    memory(plane_bytes, 3, 7)
}

fn xtrans(plane_bytes: usize) -> DemosaicMemory {
    memory(plane_bytes, 3, 22)
}

fn available_for_usable(usable: u64) -> u64 {
    (usable * 100).div_ceil(75)
}

/// A run handed decoded frames plans exactly as one that decoded them into the same bytes with no
/// transient arena — that equivalence is the whole content of `for_decoded_frames`, which exists so
/// `align_and_stack` need not encode "already decoded" as a `DemosaicMemory` with equal halves.
///
/// Checked across both tier outcomes, since the two halves feed `fits_in_ram` differently: the
/// decode peak sets one floor and the resident warped set another.
#[test]
fn a_decoded_set_plans_as_a_decode_with_no_transient() {
    let mut tiers = Vec::new();
    for (mib, frames, available) in [(4u64, 10usize, 8 * GB), (100, 40, 4 * GB)] {
        let dimensions = ImageDimensions::new(((mib * MIB) as usize / size_of::<f32>(), 1), 3);
        let frame_bytes = dimensions.sample_count() * size_of::<f32>();
        let threads = 8;

        tiers.push(
            MemoryPlan::for_decoded_frames(dimensions, frames, threads, available).fits_in_ram,
        );
        assert_eq!(
            MemoryPlan::for_decoded_frames(dimensions, frames, threads, available),
            MemoryPlan::plan(
                dimensions.pixel_count() * size_of::<f32>(),
                DemosaicMemory {
                    output_bytes: frame_bytes,
                    peak_bytes: frame_bytes,
                },
                frames,
                threads,
                available,
            ),
            "{frames} frames of {mib} MiB against {available} bytes"
        );
    }
    assert_eq!(
        tiers,
        [true, false],
        "the two cases must land on opposite sides of the tier decision"
    );
}

#[test]
fn scratch_reserve_streams_a_set_whose_frames_alone_would_fit() {
    let plane_bytes = plane(100);
    let (frames, threads, available) = (10, 8, 8 * GB);
    let demosaic = xtrans(plane_bytes);

    // The warped set alone fits; it is the per-worker scratch on top that forces the spill.
    assert!(fits_in_memory(
        PerFrameBytes::new(plane_bytes, demosaic).warped,
        frames,
        available
    ));
    assert!(!MemoryPlan::plan(plane_bytes, demosaic, frames, threads, available).fits_in_ram);
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
        assert_eq!(
            MemoryPlan::plan(plane_bytes, demosaic, 10, 8, 8 * GB),
            expected
        );
    }
}

#[test]
fn small_set_uses_all_workers_in_ram() {
    let plane_bytes = plane(10);
    assert_eq!(
        MemoryPlan::plan(plane_bytes, xtrans(plane_bytes), 5, 8, 8 * GB),
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
        MemoryPlan::plan(plane_bytes, xtrans(plane_bytes), frames, threads, boundary),
        MemoryPlan {
            fits_in_ram: true,
            decode_concurrency: 2,
            warp_concurrency: 4,
        }
    );
    for demosaic in [mono(plane_bytes), bayer(plane_bytes)] {
        let plan = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, boundary);
        assert_eq!(plan.decode_concurrency, 4);
        assert!(plan.fits_in_ram);
    }

    // A MiB under the boundary and the three-channel pair spills; mono's 47 planes still fit.
    let under = available_for_usable(569 * MIB);
    for demosaic in [bayer(plane_bytes), xtrans(plane_bytes)] {
        assert!(!MemoryPlan::plan(plane_bytes, demosaic, frames, threads, under).fits_in_ram);
    }
    assert!(MemoryPlan::plan(plane_bytes, mono(plane_bytes), frames, threads, under).fits_in_ram);

    // Headroom scales the X-Trans fan-out: 760 usable less 150 resident is 610 MiB, three 19P
    // transients' worth.
    assert_eq!(
        MemoryPlan::plan(
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

#[test]
fn planned_concurrency_never_overshoots_its_tier_budget() {
    for &plane_mib in &[16u64, 64, 100, 400] {
        let plane_bytes = plane(plane_mib);
        let memories = [mono(plane_bytes), bayer(plane_bytes), xtrans(plane_bytes)];
        for demosaic in memories {
            for &frames in &[4usize, 12, 30, 60] {
                for &threads in &[1usize, 8, 32] {
                    for &budget_gib in &[1u64, 2, 4, 8, 16] {
                        let available = budget_gib * GB;
                        let plan =
                            MemoryPlan::plan(plane_bytes, demosaic, frames, threads, available);
                        let per_frame = PerFrameBytes::new(plane_bytes, demosaic);
                        let usable = memory_budget(available);
                        let worker_cap = frames.min(threads.max(1));

                        assert!(plan.decode_concurrency <= worker_cap);
                        assert!(plan.warp_concurrency <= worker_cap);
                        assert!(plan.decode_concurrency >= 1 && plan.warp_concurrency >= 1);

                        let decode_peak = if plan.fits_in_ram {
                            (demosaic.output_bytes as u64).saturating_mul(frames as u64)
                                + (demosaic.peak_bytes.saturating_sub(demosaic.output_bytes) as u64)
                                    .saturating_mul(plan.decode_concurrency as u64)
                        } else {
                            (demosaic.peak_bytes.max(per_frame.working) as u64)
                                .saturating_mul(plan.decode_concurrency as u64)
                        };
                        let warp_peak = if plan.fits_in_ram {
                            (per_frame.warped * frames) as u64
                                + per_frame.working as u64 * plan.warp_concurrency as u64
                        } else {
                            per_frame.working as u64 * plan.warp_concurrency as u64
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

    let tight = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, 2 * GB);
    let roomy_streaming = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, 16 * GB);
    let ample = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, 1 << 50);

    assert!(!tight.fits_in_ram);
    assert!(!roomy_streaming.fits_in_ram);
    assert!(ample.fits_in_ram);
    assert!(roomy_streaming.decode_concurrency > tight.decode_concurrency);
    assert!(roomy_streaming.warp_concurrency > tight.warp_concurrency);
}
