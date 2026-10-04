//! [`PlaneStore`]: where a spilled frame's planes are written.

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FramePlane;
use crate::frame_store::stored_plane::StoredPlane;

/// Somewhere a frame's planes go to disk and come back as memory maps: the
/// [`DecodeCache`](crate::frame_store::decode_cache::DecodeCache)'s named files through a
/// [`FrameSpill`](crate::frame_store::frame_spill::FrameSpill), or
/// [`RunScratch`](crate::frame_store::run_scratch::RunScratch)'s files with no name at all.
pub(crate) trait PlaneStore {
    fn store_channel(&self, channel: usize, pixels: &[f32])
    -> Result<StoredPlane, FrameStoreError>;

    /// A quality plane: [`FramePlane::Coverage`] or [`FramePlane::Confidence`].
    fn store_quality(
        &self,
        plane: FramePlane,
        pixels: &[f32],
    ) -> Result<StoredPlane, FrameStoreError>;

    fn store_flags(&self, bytes: &[u8]) -> Result<StoredPlane<u8>, FrameStoreError>;
}
