//! What is known about one frame of a set: what its decoder stated, the conditions it was taken
//! under, its statistics and its quality planes, and how a stage reads it.

pub(crate) mod admitted;
pub(crate) mod capture_condition;
pub(crate) mod capture_conditions;
pub(crate) mod error;
pub(crate) mod frame_facts;
pub(crate) mod frame_peek;
pub(crate) mod frame_plane;
pub(crate) mod frame_quality;
pub(crate) mod frame_stats;

#[cfg(feature = "internals")]
pub(crate) mod bench;
