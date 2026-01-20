use crate::image::Image;
use crate::prelude::*;

/// Returns the workspace root directory (parent of the crate directory).
fn workspace_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/..")
}

/// Returns the path to a test resource file relative to workspace root.
fn test_resource(name: &str) -> String {
    format!("{}/test_resources/{}", workspace_root(), name)
}

/// Loads the lena test image as RGBA_U8 format.
pub fn load_lena_rgba_u8() -> Image {
    Image::read_file(test_resource("lena.tiff"))
        .unwrap()
        .convert(ColorFormat::from((
            ChannelCount::Rgba,
            ChannelSize::_8bit,
            ChannelType::UInt,
        )))
        .unwrap()
}
