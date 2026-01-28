//! Common utilities for lumos.

#[cfg(target_arch = "x86_64")]
pub mod cpu_features;

mod parallel;

pub use parallel::parallel_chunked;
