//! Where a set's frames live: in memory or in files a run maps back, and in the decode cache a run
//! keeps for the next. What a run may hold, and the plan that keeps it there, is in `memory`.

pub(crate) mod decode_cache;
pub(crate) mod disk_root;
pub(crate) mod error;
pub(crate) mod run_scratch;
pub(crate) mod run_storage;
pub(crate) mod stored_frame;
pub(crate) mod stored_gain;
pub(crate) mod stored_image;
pub(crate) mod stored_layout;
pub(crate) mod stored_plane;
pub(crate) mod stratified_samples;
pub(crate) mod tier;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
