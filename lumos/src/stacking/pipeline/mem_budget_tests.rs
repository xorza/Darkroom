//! Deterministic tests for the raw-light pipeline's memory tier and concurrency arithmetic.

use crate::io::raw::demosaic::DemosaicMemory;
use crate::resources::memory_budget;
use crate::stacking::frame_store::{
    MemoryPlan, PER_FRAME_WORKING_PLANES, fits_in_memory, plan_memory,
};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

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

#[test]
fn scratch_reserve_streams_a_set_whose_frames_alone_would_fit() {
    let plane_bytes = plane(100);
    let (frames, threads, available) = (10, 8, 8 * GIB);

    assert!(fits_in_memory(4 * plane_bytes, frames, available));
    assert!(
        !plan_memory(plane_bytes, xtrans(plane_bytes), frames, threads, available,).fits_in_ram
    );
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
        assert_eq!(plan_memory(plane_bytes, demosaic, 10, 8, 8 * GIB), expected);
    }
}

#[test]
fn small_set_uses_all_workers_in_ram() {
    let plane_bytes = plane(10);
    assert_eq!(
        plan_memory(plane_bytes, xtrans(plane_bytes), 5, 8, 8 * GIB),
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
    let frames = 5;
    let threads = 4;

    // At 520 MiB usable, resident/working memory is exactly 4P×5 + 8P×4 = 520 MiB.
    // X-Trans then has 370 MiB beyond its 3P×5 outputs: enough for one 19P transient.
    let one_worker = plan_memory(
        plane_bytes,
        xtrans(plane_bytes),
        frames,
        threads,
        available_for_usable(520 * MIB),
    );
    assert_eq!(
        one_worker,
        MemoryPlan {
            fits_in_ram: true,
            decode_concurrency: 1,
            warp_concurrency: 4,
        }
    );

    // Ten more usable MiB makes that headroom exactly 2 × 19P = 380 MiB.
    let two_workers = plan_memory(
        plane_bytes,
        xtrans(plane_bytes),
        frames,
        threads,
        available_for_usable(530 * MIB),
    );
    assert_eq!(two_workers.decode_concurrency, 2);

    for demosaic in [mono(plane_bytes), bayer(plane_bytes)] {
        let plan = plan_memory(
            plane_bytes,
            demosaic,
            frames,
            threads,
            available_for_usable(520 * MIB),
        );
        assert_eq!(plan.decode_concurrency, 4);
        assert!(plan.fits_in_ram);
    }
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
                        let available = budget_gib * GIB;
                        let plan = plan_memory(plane_bytes, demosaic, frames, threads, available);
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
                            (demosaic
                                .peak_bytes
                                .max(PER_FRAME_WORKING_PLANES * plane_bytes)
                                as u64)
                                .saturating_mul(plan.decode_concurrency as u64)
                        };
                        let warp_peak = if plan.fits_in_ram {
                            (4 * plane_bytes * frames) as u64
                                + (PER_FRAME_WORKING_PLANES * plane_bytes) as u64
                                    * plan.warp_concurrency as u64
                        } else {
                            (PER_FRAME_WORKING_PLANES * plane_bytes) as u64
                                * plan.warp_concurrency as u64
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

    let tight = plan_memory(plane_bytes, demosaic, frames, threads, 2 * GIB);
    let roomy_streaming = plan_memory(plane_bytes, demosaic, frames, threads, 16 * GIB);
    let ample = plan_memory(plane_bytes, demosaic, frames, threads, 1 << 50);

    assert!(!tight.fits_in_ram);
    assert!(!roomy_streaming.fits_in_ram);
    assert!(ample.fits_in_ram);
    assert!(roomy_streaming.decode_concurrency > tight.decode_concurrency);
    assert!(roomy_streaming.warp_concurrency > tight.warp_concurrency);
}
