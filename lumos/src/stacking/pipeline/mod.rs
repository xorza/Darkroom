//! End-to-end registered stacking orchestration.
//!
//! [`align::align_and_stack`] detects, registers, warps, and combines calibrated images.
//! [`streaming::calibrate_align_stack`] prepends RAW calibration and chooses the RAM or
//! memory-bounded disk tier.

pub(crate) mod align;
pub(crate) mod config;
pub(crate) mod result;
pub(crate) mod streaming;

#[cfg(test)]
mod mem_budget_probe;
#[cfg(test)]
mod mem_budget_tests;
#[cfg(test)]
mod tests;
