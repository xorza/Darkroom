//! Memory planning and RAM/mmap storage shared by stacking stages.

pub(crate) mod cache_key;
pub(crate) mod error;
pub(crate) mod frame_facts;
pub(crate) mod frame_peek;
pub(crate) mod frame_quality;
pub(crate) mod frame_spill;
pub(crate) mod frame_stats;
pub(crate) mod spill_directory;
pub(crate) mod stackable_image;
pub(crate) mod stored_frame;
pub(crate) mod stored_image;
pub(crate) mod stored_plane;

#[cfg(test)]
mod tests;
