//! Why a published value has no image to draw.

use palantir::RegisterImageError;

/// Why a preview value could not become a texture.
///
/// Terminal and stored: an entry keeps its failure rather than retrying the
/// conversion on every frame that draws it, so a broken value is diagnosed
/// once and reported the same way from then on.
#[derive(Clone, Debug, thiserror::Error)]
pub(crate) enum PreviewImageError {
    /// The node published something that isn't an image — a number, a string.
    /// Not a fault: the preview card formats it, and only a viewer tab, which
    /// has nothing else to show, reports it.
    #[error("value is not an image")]
    NotAnImage,
    #[error("image is empty")]
    Empty,
    /// The pixels could not be read back to the CPU.
    ///
    /// Rendered at construction rather than held as the `imaginarium::Error`
    /// it came from: that type is not `Clone`, and this one has to be — a
    /// viewer reads it out from behind the store's cell rather than borrowing
    /// it.
    #[error("could not read image pixels: {0}")]
    Pixels(String),
    /// The renderer refused the texture — larger than the device's maximum
    /// 2D dimension.
    #[error("{0}")]
    Register(#[from] RegisterImageError),
}
