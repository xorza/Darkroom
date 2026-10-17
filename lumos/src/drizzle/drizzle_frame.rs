//! [`DrizzleFrame`]: one drizzle input, with what has to stay aligned with it.

use imaginarium::Buffer2;

use crate::registration::warp_transform::WarpTransform;

/// One drizzle input and all metadata that must remain aligned with it.
#[derive(Debug, Clone)]
pub struct DrizzleFrame<T> {
    /// Image or path to load: a demosaiced or mono frame, or a calibrated mosaic whose photosites
    /// each reach the channel of their color alone.
    pub source: T,
    /// The registration's warp, from the common reference grid to this input — what
    /// [`RegistrationResult::warp_transform`](crate::RegistrationResult::warp_transform) returns.
    /// Drizzle maps input pixels through its inverse, SIP included.
    pub warp: WarpTransform,
    /// Optional non-negative per-pixel quality weights with the same dimensions as the image.
    pub pixel_weight_map: Option<Buffer2<f32>>,
}

impl<T> DrizzleFrame<T> {
    /// A frame without a per-pixel weight map.
    pub const fn new(source: T, warp: WarpTransform) -> Self {
        Self {
            source,
            warp,
            pixel_weight_map: None,
        }
    }
}
