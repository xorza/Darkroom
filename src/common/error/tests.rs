use std::error::Error as _;
use std::io;

use crate::common::error::Error;

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
    ] {
        assert_eq!(error.to_string(), message);
        assert_eq!(error.source().unwrap().to_string(), cause);
    }

    assert!(
        Error::InvalidExtension("xyz".to_string())
            .source()
            .is_none()
    );
}
