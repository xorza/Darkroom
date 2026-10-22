//! [`PlaneSink`]: where a stored frame's planes are written.

use bytemuck::Pod;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::stored_plane::StoredPlane;

/// Where a frame's planes are written and mapped back from: the run's scratch, or a kept frame's
/// files in the decode cache, which names each plane by its role.
pub(crate) trait PlaneSink {
    /// `pixels`, the plane of `role`, written and mapped back.
    fn write<T: Pod>(
        &mut self,
        role: PlaneRole,
        pixels: &[T],
    ) -> Result<StoredPlane<T>, FrameStoreError>;
}

impl<S: PlaneSink> PlaneSink for &mut S {
    fn write<T: Pod>(
        &mut self,
        role: PlaneRole,
        pixels: &[T],
    ) -> Result<StoredPlane<T>, FrameStoreError> {
        (**self).write(role, pixels)
    }
}

/// What a plane of a stored frame holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneRole {
    Channel(usize),
    Flags,
    /// A slot's stratified samples.
    Samples(usize),
    /// A channel's flat gain nodes.
    Gain(usize),
    /// A warp's or a drizzle's quality plane.
    Quality,
}
