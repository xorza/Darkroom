//! [`MaskedPlane`]: a plane, less the pixels a mask names.

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;

/// A plane, less the pixels `mask` names: what a repair or a background reads from it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MaskedPlane<'a> {
    pub(crate) pixels: &'a Buffer2<f32>,
    pub(crate) mask: Option<&'a BitBuffer2>,
}

/// The whole plane, no pixel left out.
impl<'a> From<&'a Buffer2<f32>> for MaskedPlane<'a> {
    fn from(pixels: &'a Buffer2<f32>) -> Self {
        Self { pixels, mask: None }
    }
}
