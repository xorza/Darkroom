pub(crate) mod cache;
pub(crate) mod cache_config;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod normalization;
pub(crate) mod rejection;
pub(crate) mod stack;

/// Coverage below this threshold is dominated by warp border fill.
pub(crate) const MIN_CONTRIBUTING_COVERAGE: f32 = 1e-3;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod mem_budget_probe;
#[cfg(test)]
mod mem_budget_tests;
#[cfg(all(test, feature = "real-data"))]
mod real_data_tests;
#[cfg(test)]
mod synthetic_tests;
