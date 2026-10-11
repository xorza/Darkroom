//! How much of the machine the pipeline is willing to use, and what that buys.
//!
//! One [`Budget`](budget::Budget), read once at a run's entry from its
//! [`RunMemory`](run_memory::RunMemory), and one rule over it: a run is a list of
//! [`Stage`](stage::Stage)s, each priced in what it keeps of a frame, holds beside the frames and
//! adds per unit of work in flight, and [`RunPlan`](run_plan::RunPlan) decides from them whether
//! the frames stay resident and how wide each stage runs. A frame's way through a stage is priced
//! as a chain of [`Footprint`](footprint::Footprint)s, each step's from the component that takes
//! it.

pub(crate) mod budget;
pub(crate) mod footprint;
pub(crate) mod run_memory;
pub(crate) mod run_plan;
pub(crate) mod stage;

/// Share of available RAM the pipeline will commit, leaving the rest as headroom for allocator
/// slack, the OS page cache, and whatever else the machine is doing.
const MEMORY_PERCENT: u64 = 75;

pub(crate) fn memory_budget(available_memory: u64) -> u64 {
    (u128::from(available_memory) * u128::from(MEMORY_PERCENT) / 100) as u64
}

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
