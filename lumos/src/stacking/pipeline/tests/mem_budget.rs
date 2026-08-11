//! Deterministic tests for the raw-light pipeline's memory tier and concurrency arithmetic.

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::memory::{MemoryPlan, PerFrameBytes, fits_in_memory, memory_budget};

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

/// A run handed decoded frames plans exactly as one that decoded them into the same bytes with no
/// transient arena — that equivalence is the whole content of `for_decoded_frames`, which exists so
/// `align_and_stack` need not encode "already decoded" as a `DemosaicMemory` with equal halves.
///
/// Checked across both tier outcomes, since the two halves feed `fits_in_ram` differently: the
/// decode peak sets one floor and the resident warped set another.
#[test]
fn a_decoded_set_plans_as_a_decode_with_no_transient() {
    let mut tiers = Vec::new();
    for (mib, frames, available) in [(4u64, 10usize, 8 * GIB), (100, 40, 4 * GIB)] {
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
    let (frames, threads, available) = (10, 8, 8 * GIB);
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
            MemoryPlan::plan(plane_bytes, demosaic, 10, 8, 8 * GIB),
            expected
        );
    }
}

#[test]
fn small_set_uses_all_workers_in_ram() {
    let plane_bytes = plane(10);
    assert_eq!(
        MemoryPlan::plan(plane_bytes, xtrans(plane_bytes), 5, 8, 8 * GIB),
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
                        let available = budget_gib * GIB;
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

    let tight = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, 2 * GIB);
    let roomy_streaming = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, 16 * GIB);
    let ample = MemoryPlan::plan(plane_bytes, demosaic, frames, threads, 1 << 50);

    assert!(!tight.fits_in_ram);
    assert!(!roomy_streaming.fits_in_ram);
    assert!(ample.fits_in_ram);
    assert!(roomy_streaming.decode_concurrency > tight.decode_concurrency);
    assert!(roomy_streaming.warp_concurrency > tight.warp_concurrency);
}
