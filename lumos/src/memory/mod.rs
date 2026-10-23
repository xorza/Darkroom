//! How much of the machine the pipeline is willing to use, and what that buys.
//!
//! One [`Budget`](budget::Budget), read once at a run's entry, and one rule over it: a run is a
//! list of [`Stage`](stage::Stage)s, each priced in what it keeps of a frame, holds beside the
//! frames and adds per unit of work in flight, and [`RunPlan`](run_plan::RunPlan) decides from
//! them whether the frames stay resident and how wide each stage runs. A frame's way through a stage is priced
//! as a chain of [`Footprint`](footprint::Footprint)s, each step's from the component that takes
//! it.

pub(crate) mod budget;
pub(crate) mod footprint;
pub(crate) mod run_plan;
pub(crate) mod stage;

/// Share of available RAM the pipeline will commit, leaving the rest as headroom for allocator
/// slack, the OS page cache, and whatever else the machine is doing.
const MEMORY_PERCENT: u64 = 75;

/// The bytes of `available` the pipeline commits: [`MEMORY_PERCENT`] of it, rounded down.
pub(crate) fn usable(available: u64) -> u64 {
    u64::try_from(u128::from(available) * u128::from(MEMORY_PERCENT) / 100)
        .expect("a share below 100% of a u64 fits a u64")
}

#[cfg(test)]
mod tests;
