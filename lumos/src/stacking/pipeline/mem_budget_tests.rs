//! Whether the raw-light pipeline's memory-tier decision respects its budget — the deterministic
//! guard behind [`plan_memory`]'s contract: take the all-in-memory path only when the warped frames
//! *and* the RAM path's per-frame scratch fit ~75% RAM, else stream through the disk cache with a
//! fan-out that keeps peak load heap inside the budget. No live measurement.
//!
//! The at-scale peak-RSS counterparts live one level down, in the stages this pipeline drives: the
//! combine's `master_stack_memory_probe` (the streaming combine that finishes the stack) and star
//! detection's `detect_memory_probe` (the per-frame stage the streaming loop runs). Here we pin the
//! arithmetic those probes can only observe end-to-end: the tier flip and the streaming concurrency.
//!
//! [`plan_memory`]: super::plan_memory

use super::{MemoryPlan, PER_FRAME_DECODE_PLANES, PER_FRAME_WORKING_PLANES, plan_memory};
use crate::stacking::combine::cache_config::{fits_in_memory, memory_budget};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// A single-channel f32 plane of `mib` MiB, in bytes — the pipeline's `plane_bytes`.
fn plane(mib: u64) -> usize {
    (mib * MIB) as usize
}

/// The scratch reserve is load-bearing: a set whose warped frames alone fit ~75% RAM must still be
/// streamed when the RAM path's per-frame detection/warp scratch wouldn't also fit — the overshoot
/// this reserve exists to prevent. Hand-computed: 100 MB planes, 10 frames, 8 workers, 8 GiB budget.
#[test]
fn scratch_reserve_streams_a_set_whose_frames_alone_would_fit() {
    let plane_bytes = plane(100);
    let (frames, threads, available) = (10, 8, 8 * GIB);

    // Frames alone: 4 planes × 100 MB × 10 = 4000 MB ≤ 6144 MB usable (0.75 × 8 GiB) → they fit.
    let warped_bytes = 4 * plane_bytes;
    assert!(
        fits_in_memory(warped_bytes, frames, available),
        "precondition: the warped frames alone must fit, so streaming is the reserve's doing"
    );

    // With scratch: concurrent = min(10, 8) = 8; reserve = 8 planes × 100 MB × 8 = 6400 MB, leaving
    // 8192 − 6400 = 1792 MB → 1344 MB usable < 4000 MB → must stream.
    let plan = plan_memory(plane_bytes, frames, threads, available);
    assert!(
        !plan.fits_in_ram,
        "frames fit but frames + scratch don't → must stream, got fits_in_ram = true"
    );
}

/// The exact plan for that pinned case: streaming, decode fanning out to 4 and warp to 7 (decode
/// carries the extra demosaic arena, so it runs fewer in flight). usable = 6144 MB; decode transient
/// = 14 × 100 = 1400 MB → ⌊6144/1400⌋ = 4; warp transient = 8 × 100 = 800 MB → ⌊6144/800⌋ = 7; both
/// under the 8-worker cap.
#[test]
fn plan_memory_computes_exact_streaming_concurrencies() {
    let plan = plan_memory(plane(100), 10, 8, 8 * GIB);
    assert_eq!(
        plan,
        MemoryPlan {
            fits_in_ram: false,
            decode_concurrency: 4,
            warp_concurrency: 7,
        }
    );
}

/// A small set fits entirely in RAM: 10 MB planes, 5 frames, 8 GiB budget. Warped total = 4 × 10 × 5
/// = 200 MB; reserve = 8 × 10 × 5 = 400 MB → frame budget 7792 MB → 5844 MB usable ≫ 200 MB → RAM.
#[test]
fn small_set_takes_the_ram_tier() {
    let plan = plan_memory(plane(10), 5, 8, 8 * GIB);
    assert!(
        plan.fits_in_ram,
        "a 200 MB warped set on 8 GiB must stay in RAM"
    );
}

/// The streaming loader's guarantee, as an invariant sweep: for any (plane size, frame count, worker
/// count, budget), neither step's chosen concurrency lets projected peak load heap exceed the usable
/// budget — except at the floor where not even one frame's transient fits (concurrency pinned to 1).
/// Also pins two structural facts: decode never fans out wider than warp (it carries the heavier
/// per-frame transient), and both stay within the worker cap. Fails if a divisor ever reverts to the
/// resident frame size (the ~2× overshoot bug the combine test guards at its own level).
#[test]
fn streaming_concurrency_never_overshoots_budget() {
    for &plane_mib in &[16u64, 64, 100, 400] {
        let plane_bytes = plane(plane_mib);
        for &frames in &[4usize, 12, 30, 60] {
            for &threads in &[1usize, 8, 32] {
                for &budget_gib in &[1u64, 2, 4, 8, 16] {
                    let available = budget_gib * GIB;
                    let plan = plan_memory(plane_bytes, frames, threads, available);
                    let usable = memory_budget(available);

                    // Structural: decode carries more planes per in-flight frame than warp, so it can
                    // never run more of them at once; both honor the worker cap and never hit zero.
                    assert!(
                        plan.decode_concurrency <= plan.warp_concurrency,
                        "decode ({}) must not out-fan warp ({}) — decode holds the heavier transient",
                        plan.decode_concurrency,
                        plan.warp_concurrency
                    );
                    assert!(plan.warp_concurrency <= threads.max(1));
                    assert!(plan.decode_concurrency >= 1 && plan.warp_concurrency >= 1);

                    // Budget: projected peak = concurrency × per-decode transient ≤ usable, unless
                    // pinned to the 1-frame floor where even one transient overflows the budget.
                    let decode_peak = plan.decode_concurrency as u64
                        * (PER_FRAME_DECODE_PLANES * plane_bytes) as u64;
                    let warp_peak = plan.warp_concurrency as u64
                        * (PER_FRAME_WORKING_PLANES * plane_bytes) as u64;
                    assert!(
                        decode_peak <= usable || plan.decode_concurrency == 1,
                        "plane={plane_mib}MB frames={frames} threads={threads} budget={budget_gib}GB: \
                         decode peak {}MB > {}MB usable",
                        decode_peak / MIB,
                        usable / MIB
                    );
                    assert!(
                        warp_peak <= usable || plan.warp_concurrency == 1,
                        "plane={plane_mib}MB frames={frames} threads={threads} budget={budget_gib}GB: \
                         warp peak {}MB > {}MB usable",
                        warp_peak / MIB,
                        usable / MIB
                    );
                }
            }
        }
    }
}

/// Budget drives the tier and the fan-out: for a fixed frame set, growing the RAM budget can only
/// flip streaming → in-memory (never back) and never lowers either streaming concurrency. Pins that
/// the knob actually moves the decision. 100 MB planes, 20 frames, 16 workers.
#[test]
fn more_budget_flips_to_ram_and_never_lowers_concurrency() {
    let plane_bytes = plane(100);
    let (frames, threads) = (20, 16);

    // Tiny budget streams (20 × 400 MB = 8000 MB warped ≫ 1536 MB usable); 1 PiB keeps all resident.
    let tight = plan_memory(plane_bytes, frames, threads, 2 * GIB);
    let ample = plan_memory(plane_bytes, frames, threads, 1 << 50); // 1 PiB — no overflow in ×75
    assert!(
        !tight.fits_in_ram,
        "20 × 400 MB warped on 2 GiB must stream"
    );
    assert!(ample.fits_in_ram, "the same set on 1 PiB must stay in RAM");

    // Concurrency is monotonic non-decreasing in the budget, up to the worker cap. usable 3072 MB →
    // ⌊3072/800⌋ = 3 warp; usable 12288 MB → ⌊12288/800⌋ = 15 warp — strictly more.
    let lo = plan_memory(plane_bytes, frames, threads, 4 * GIB);
    let hi = plan_memory(plane_bytes, frames, threads, 16 * GIB);
    assert!(hi.decode_concurrency >= lo.decode_concurrency);
    assert!(hi.warp_concurrency >= lo.warp_concurrency);
    assert!(
        hi.warp_concurrency > lo.warp_concurrency || hi.warp_concurrency == threads,
        "more RAM must raise warp concurrency until it saturates the worker cap: {} vs {}",
        lo.warp_concurrency,
        hi.warp_concurrency
    );
}
