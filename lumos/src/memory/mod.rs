//! How much of the machine the pipeline is willing to use, and what that buys.
//!
//! One budget ([`memory_budget`]) and everything derived from it: the row chunk a combine reads at
//! a time ([`ChunkMemoryLayout`](chunk_memory_layout::ChunkMemoryLayout)), how many frames may
//! decode or warp concurrently, and the resident-vs-spilled tier decision
//! ([`MemoryPlan`](memory_plan::MemoryPlan)). Every caller sizes its work against this file rather than
//! against `available_memory` directly, through the one [`RunMemory`](run_memory::RunMemory) its
//! run read at the entry.

pub(crate) mod cgroup_memory;
pub(crate) mod chunk_memory_layout;
pub(crate) mod memory_plan;
pub(crate) mod run_memory;

use std::sync::LazyLock;

use crate::memory::cgroup_memory::CgroupMemory;
use crate::mount_table::MountTable;
use std::sync::PoisonError;

/// Share of available RAM the pipeline will commit, leaving the rest as headroom for allocator
/// slack, the OS page cache, and whatever else the machine is doing.
const MEMORY_PERCENT: u64 = 75;

/// What the process can allocate: the host's available memory, or less where its control groups
/// leave less ([`CgroupMemory`]).
pub(crate) fn available_memory() -> u64 {
    use std::sync::Mutex;
    use sysinfo::System;

    // Where the groups are is read once; what they hold, on every call.
    static GROUPS: LazyLock<Option<CgroupMemory>> =
        LazyLock::new(|| CgroupMemory::of_process(&MountTable::read()));

    // Reused rather than built per call. Constructing a `System` costs ~20 µs against ~5 µs to
    // refresh one that already exists, and asking for memory alone
    // (`new_with_specifics(RefreshKind::nothing().with_memory(..))`) measured no cheaper — the cost
    // is the construction itself, so reuse is the only thing that helps. Every planning decision
    // in the pipeline goes through here.
    static SYSTEM: LazyLock<Mutex<System>> = LazyLock::new(|| Mutex::new(System::new()));

    // Recover rather than propagate: a poisoned lock means some earlier caller panicked, but the
    // `System` behind it is a cache of OS counters with no invariant to corrupt.
    let mut system = SYSTEM.lock().unwrap_or_else(PoisonError::into_inner);
    system.refresh_memory();
    let available = system.available_memory();

    // macOS can report zero when compressed pages exceed free, inactive, and purgeable pages.
    let host = if available == 0 {
        system.total_memory().saturating_sub(system.used_memory())
    } else {
        available
    };
    GROUPS
        .as_ref()
        .and_then(CgroupMemory::available)
        .map_or(host, |groups| host.min(groups))
}

pub(crate) fn memory_budget(available_memory: u64) -> u64 {
    (u128::from(available_memory) * u128::from(MEMORY_PERCENT) / 100) as u64
}

/// Statistics hold a full-frame scratch buffer beside the decoded pixels.
pub(crate) const DECODE_TRANSIENT_FACTOR: usize = 2;

/// Frames that may be in flight at once: budget minus what stays resident, divided by the peak
/// one in-flight frame adds, capped at the worker count and never below one.
pub(crate) fn load_concurrency(
    resident_bytes_per_frame: usize,
    transient_bytes_per_decode: usize,
    resident_frames: usize,
    available_memory: u64,
    max_workers: usize,
) -> usize {
    let usable = memory_budget(available_memory);
    let transient = (transient_bytes_per_decode as u64).max(1);
    let resident = (resident_bytes_per_frame as u64).saturating_mul(resident_frames as u64);
    let headroom = usable.saturating_sub(resident);
    ((headroom / transient).max(1) as usize).min(max_workers.max(1))
}

/// Quality planes a frame carries beside its image: `coverage` and `confidence`, one image-sized
/// plane each. A warp emits them, and so does a decoder that found pixels the source declared no
/// measurement for — see `registration::resample::WarpResult` and `frame_store::FrameQuality`.
pub(crate) const FRAME_QUALITY_PLANES: usize = 2;

/// Image-sized planes the star detector's pool holds at its high-water mark over every preset:
/// four f32 planes (the measurement plane, its sky noise, the detection plane and the filter's
/// pass scratch, which the detection plane's noise takes over), the u32 label map, and four
/// bitmasks (saturation, threshold, the sources a refined sky was measured around, and the pixels
/// with no data). A bitmask is a thirty-second of an f32 plane; charging each as a whole one keeps
/// this integral and errs high.
///
/// `star_detection::mem_budget` pins the detector's actual pool and checks it against this,
/// so a stage that grows its scratch cannot drift away from the planner silently.
pub(crate) const DETECTION_WORKING_PLANES: usize = 9;

#[cfg(test)]
mod tests;
