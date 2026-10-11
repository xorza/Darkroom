//! How much of the machine the pipeline is willing to use, and what that buys.
//!
//! One budget ([`memory_budget`]) and everything derived from it: the row chunk a combine reads at
//! a time ([`ChunkMemoryLayout`](chunk_memory_layout::ChunkMemoryLayout)), how many frames may
//! decode or warp concurrently, and the resident-vs-spilled tier decision
//! ([`MemoryPlan`](memory_plan::MemoryPlan)). Every caller sizes its work against this file rather than
//! against the system's reading directly, through the one [`RunMemory`](run_memory::RunMemory) its
//! run read at the entry.

pub(crate) mod chunk_memory_layout;
pub(crate) mod memory_plan;
pub(crate) mod run_memory;

/// Share of available RAM the pipeline will commit, leaving the rest as headroom for allocator
/// slack, the OS page cache, and whatever else the machine is doing.
const MEMORY_PERCENT: u64 = 75;

pub(crate) fn memory_budget(available_memory: u64) -> u64 {
    (u128::from(available_memory) * u128::from(MEMORY_PERCENT) / 100) as u64
}

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

/// Quality planes a warped frame carries beside its image: `coverage` and `confidence`, one
/// image-sized plane each, channel-independent — see `registration::resample::WarpResult` and
/// `frame_store::FrameQuality`.
pub(crate) const FRAME_QUALITY_PLANES: usize = 2;

/// Image-sized planes the star detector's pool holds at its high-water mark over every preset: four
/// f32 planes (the measurement plane, its sky noise, the detection plane and the filter's pass
/// scratch, which the detection plane's noise takes over), the labeling's runs, under a plane for a
/// mask of sources, and four bitmasks (saturation, threshold, the sources a refined sky was
/// measured around, and the pixels with no data). A bitmask is a thirty-second of an f32 plane;
/// charging each as a whole one keeps this integral and errs high.
///
/// `star_detection::tests::pool_working_set` pins the detector's actual pool and checks it against this,
/// so a stage that grows its scratch cannot drift away from the planner silently.
pub(crate) const DETECTION_WORKING_PLANES: usize = 9;

#[cfg(test)]
mod tests;
