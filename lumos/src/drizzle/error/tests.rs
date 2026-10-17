use std::io;
use std::path::{Path, PathBuf};

use crate::drizzle::error::*;
use crate::image::channel_layout::ChannelLayout;
use crate::image::error::FrameDimensionMismatch;
use crate::image::image_dimensions::ImageDimensions;

/// The shared failures read the same from a drizzle as from a stack, their wording kept with
/// the payload that owns it.
#[test]
fn shared_failures_report_through_the_stack_error() {
    assert_eq!(
        DrizzleError::from(StackError::NoFrames).to_string(),
        "no frames provided"
    );
    let mismatch = FrameDimensionMismatch::check(
        2,
        ImageDimensions::new((100, 100), ChannelLayout::Rgb),
        ImageDimensions::new((200, 100), ChannelLayout::Rgb),
    )
    .unwrap_err();
    assert_eq!(
        DrizzleError::from(AdmissionError::from(mismatch)).to_string(),
        "frame 2 is 200x100x3, expected 100x100x3"
    );
}

/// A cancel is one case whichever phase met it — a decode, an admission check, the combine —
/// and any other decode failure is the file's, its error kept whole.
#[test]
fn a_cancel_is_one_case_from_every_phase() {
    let cancels = [
        DrizzleError::from(ImageError::cancelled(Path::new("light.fits"))),
        DrizzleError::from(AdmissionError::Cancelled),
        DrizzleError::from(StackError::Cancelled),
    ];
    for cancel in cancels {
        assert!(cancel.is_cancelled(), "{cancel:?}");
    }
    let missing = ImageError::Io {
        path: PathBuf::from("light.fits"),
        source: io::Error::from(io::ErrorKind::NotFound),
    };
    assert!(matches!(
        DrizzleError::from(missing),
        DrizzleError::Stack(StackError::ImageLoad(ImageError::Io { path, .. }))
            if path == Path::new("light.fits")
    ));
}
