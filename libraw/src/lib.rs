//! A safe wrapper over LibRaw: a camera RAW file opened from memory, its sensor data unpacked,
//! what LibRaw states of it as plain Rust values, and LibRaw's own processing.
//!
//! Every `unsafe` call into LibRaw sits in this crate: [`Libraw`] owns the handle, and its
//! methods read LibRaw's state into [`SensorLayout`], [`ImageData`], [`ColorData`], [`Capture`]
//! and [`CodecFacts`]. What a reader makes of those values — a mosaic, a black level, a refusal —
//! is the reader's.

mod capture;
mod codec_facts;
mod color_data;
mod error;
mod image_data;
mod libraw;
mod process_params;
mod processed_image;
mod sensor_layout;
mod shim;
#[expect(
    clippy::allow_attributes,
    reason = "the lints the bindings trip change with the bindgen version and the target, so an \
              `expect` there goes unfulfilled on some build"
)]
#[allow(
    warnings,
    clippy::all,
    reason = "bindgen output, named after LibRaw's C headers"
)]
mod sys;

#[cfg(feature = "internals")]
pub use crate::libraw::internals;

pub use crate::capture::Capture;
pub use crate::codec_facts::{CodecFacts, CrxCoding};
pub use crate::color_data::ColorData;
pub use crate::error::{LibrawCode, LibrawError};
pub use crate::image_data::ImageData;
pub use crate::libraw::Libraw;
pub use crate::process_params::ProcessParams;
pub use crate::processed_image::{ProcessedImage, ProcessedSamples};
pub use crate::sensor_layout::SensorLayout;

/// The length of LibRaw's `cblack` table: four channel values, the spatial pattern's height and
/// width, and the pattern.
pub const CBLACK_LEN: usize = sys::LIBRAW_CBLACK_SIZE as usize;

/// The length of LibRaw's linearization curve: a value for each 16-bit raw code.
pub const CURVE_LEN: usize = 1 << 16;

/// LibRaw's `filters` word for the 6 × 6 X-Trans layout, which it keeps apart in
/// [`ImageData::xtrans`].
pub const XTRANS_FILTERS: u32 = sys::LIBRAW_XTRANS;
