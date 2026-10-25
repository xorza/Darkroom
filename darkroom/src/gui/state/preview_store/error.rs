//! Why a published value has no image to draw.

use common::Size2us;
use palantir::ImageTooLarge;

/// Why a preview value could not become a texture.
///
/// Terminal and stored: an entry keeps its failure rather than retrying the
/// conversion on every frame that draws it, so a broken value is diagnosed
/// once and reported the same way from then on.
#[derive(Clone, Debug, thiserror::Error)]
pub(crate) enum DrawablePreviewError {
    /// The node published something that isn't an image — a number, a string.
    /// Not a fault: the preview card formats it, and only a viewer tab, which
    /// has nothing else to show, reports it.
    #[error("value is not an image")]
    NotAnImage,
    /// An edge longer than `u32::MAX` pixels, which no texture can hold.
    #[error("image is {size}, past the longest texture edge")]
    EdgeTooLong { size: Size2us },
    /// The renderer refused the texture — larger than the device's maximum
    /// 2D dimension.
    #[error("{0}")]
    Register(#[from] ImageTooLarge),
}
