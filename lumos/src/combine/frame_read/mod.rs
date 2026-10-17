//! Reading the stored frames of a combine: which slot a pixel of a channel is, which frames are
//! gathered at it, and a chunk of their rows.

pub(crate) mod frame_chunk;
pub(crate) mod frame_gate;
pub(crate) mod slots;
