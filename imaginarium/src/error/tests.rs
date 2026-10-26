use std::error::Error as _;
use std::io;

use common::Size2us;

use crate::codec::raster_format::RasterFormat;
use crate::color::color_format::ColorFormat;
use crate::error::{CodecError, DescError, TiffSampleType};
use crate::interleaved::interleaved_desc::InterleavedDesc;

/// A wrapped foreign error is the source and its own message; a case of imaginarium's own has no
/// source.
#[test]
fn only_a_wrapped_cause_is_reachable_through_source() {
    let truncated = || io::Error::new(io::ErrorKind::UnexpectedEof, "truncated");

    for (error, message, cause) in [
        (
            CodecError::from(image::ImageError::IoError(truncated())),
            "PNG or JPEG codec error: truncated",
            "truncated",
        ),
        (
            CodecError::from(tiff::TiffError::IoError(truncated())),
            "TIFF codec error: truncated",
            "truncated",
        ),
        (
            CodecError::from(DescError::ZeroWidth),
            "the file states no image: width must be positive",
            "width must be positive",
        ),
    ] {
        assert_eq!(error.to_string(), message);
        assert_eq!(error.source().unwrap().to_string(), cause);
    }

    let desc = InterleavedDesc::new(Size2us::new(3, 2), ColorFormat::RGB_F32);
    for (error, message) in [
        (
            CodecError::Unstorable {
                format: RasterFormat::Png,
                color_format: ColorFormat::RGB_F32,
            },
            "PNG cannot store RGB f32",
        ),
        (
            CodecError::TooLarge { desc },
            "3x2 RGB f32 is larger than an image file can hold",
        ),
        (
            CodecError::TiffSamples(TiffSampleType::F64),
            "unsupported TIFF sample type f64",
        ),
    ] {
        assert_eq!(error.to_string(), message);
        assert!(error.source().is_none());
    }
}
