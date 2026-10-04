//! RAM/mmap frame storage shared by stacking stages. What a run may hold, and the plan that keeps
//! it there, is in `memory`.

pub(crate) mod cache_key;
pub(crate) mod decode_cache;
pub(crate) mod disk_root;
pub(crate) mod error;
pub(crate) mod frame_facts;
pub(crate) mod frame_peek;
pub(crate) mod frame_quality;
pub(crate) mod frame_spill;
pub(crate) mod frame_stats;
pub(crate) mod plane_store;
pub(crate) mod run_scratch;
pub(crate) mod stackable_image;
pub(crate) mod stored_frame;
pub(crate) mod stored_image;
pub(crate) mod stored_plane;

#[cfg(test)]
mod tests;
