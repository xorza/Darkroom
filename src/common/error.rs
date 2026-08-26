use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid file extension: {0}")]
    InvalidExtension(String),
    #[error("Unsupported color type: {0}")]
    UnsupportedColorType(String),
    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("Invalid color format: {0}")]
    InvalidColorFormat(String),
    #[error("Size mismatch: {0}")]
    SizeMismatch(String),
    #[error("Conversion error: {0}")]
    Conversion(#[from] bytemuck::PodCastError),
    #[error("Image codec error: {0}")]
    ImageCodec(#[from] image::ImageError),
    #[error("TIFF codec error: {0}")]
    TiffCodec(#[from] tiff::TiffError),
    #[error("GPU error: {0}")]
    Gpu(String),
    #[error("GPU context not available")]
    NoGpuContext,
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn only_a_wrapped_cause_is_reachable_through_source() {
        let truncated = || io::Error::new(io::ErrorKind::UnexpectedEof, "truncated");

        for (error, message, cause) in [
            (
                Error::from(image::ImageError::IoError(truncated())),
                "Image codec error: truncated",
                "truncated",
            ),
            (
                Error::from(tiff::TiffError::IoError(truncated())),
                "TIFF codec error: truncated",
                "truncated",
            ),
            (
                Error::from(bytemuck::PodCastError::AlignmentMismatch),
                "Conversion error: AlignmentMismatch",
                "AlignmentMismatch",
            ),
        ] {
            assert_eq!(error.to_string(), message);
            assert_eq!(error.source().unwrap().to_string(), cause);
        }

        assert!(Error::NoGpuContext.source().is_none());
        assert!(
            Error::InvalidExtension("xyz".to_string())
                .source()
                .is_none()
        );
    }
}
