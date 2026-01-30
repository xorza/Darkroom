//! Common utilities for lumos.

pub(crate) mod cpu_features;
pub(crate) mod parallel;

mod buffer2;

pub use buffer2::Buffer2;
pub use parallel::parallel_chunked;
