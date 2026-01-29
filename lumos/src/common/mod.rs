//! Common utilities for lumos.

pub(crate) mod cpu_features;

pub(crate) mod buffer2;
mod parallel;

pub use parallel::parallel_chunked;
