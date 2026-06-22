mod image_data;
mod io;
mod tiff;

#[cfg(test)]
mod tests;

use std::path::Path;

use aligned_vec::{AVec, ConstAlign};

/// Supported image file extensions for reading and writing.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tiff", "tif"];

use crate::common::color_format::ColorFormat;
use crate::common::conversion::convert_image;
use crate::common::error::{Error, Result};

/// Alignment of the pixel buffer. The only hard requirement is that `u16`/`f32`
/// channels can be read via `bytemuck::cast_slice`, which panics unless the
/// buffer is aligned to the element type; `align_of::<f32>()` (4) covers every
/// supported element type (`u8`/`u16`/`f32`). The SIMD kernels all use unaligned
/// loads, so nothing benefits from over-aligning further.
const ALIGNMENT: usize = std::mem::align_of::<f32>();

/// Image dimensions + pixel format. Pixel data is **always tightly packed**
/// (`row_bytes == width * bytes_per_pixel`, no inter-row padding) — any row
/// alignment a GPU backend needs lives inside `GpuImage` (`src/gpu/`), never here.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub width: usize,
    pub height: usize,
    pub color_format: ColorFormat,
}

/// An image: a tightly-packed, `ALIGNMENT`-aligned byte buffer plus the
/// [`ImageDesc`] that says how to interpret it. The bytes are reinterpreted as
/// `u8`/`u16`/`f32` per the format via `bytemuck::cast_slice` at the use sites.
#[derive(Clone, Debug)]
pub struct Image {
    pub desc: ImageDesc,
    bytes: AVec<u8, ConstAlign<ALIGNMENT>>,
}

impl Image {
    /// Returns the image bytes as a slice.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Convert to owned bytes. Copies: an `AVec`'s over-aligned allocation cannot be
    /// freed as a plain `Vec` (the dealloc `Layout` alignment would mismatch the alloc).
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes.to_vec()
    }

    /// Returns the image bytes as a mutable slice.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    pub fn new_black(desc: ImageDesc) -> Result<Image> {
        desc.validate()?;

        let mut bytes = AVec::with_capacity(ALIGNMENT, desc.size_in_bytes());
        bytes.resize(desc.size_in_bytes(), 0);

        Ok(Image { desc, bytes })
    }

    pub fn new_with_data(desc: ImageDesc, bytes: Vec<u8>) -> Result<Image> {
        desc.validate()?;

        if bytes.len() != desc.size_in_bytes() {
            return Err(Error::SizeMismatch(format!(
                "bytes length {} does not match expected size {}",
                bytes.len(),
                desc.size_in_bytes()
            )));
        }

        Ok(Image {
            desc,
            bytes: vec_to_avec(bytes),
        })
    }

    pub fn read_file<P: AsRef<Path>>(filename: P) -> Result<Image> {
        let extension = filename
            .as_ref()
            .extension()
            .and_then(|os_str| os_str.to_str())
            .ok_or_else(|| Error::InvalidExtension("missing extension".to_string()))?
            .to_ascii_lowercase();

        let image = match extension.as_str() {
            "png" | "jpeg" | "jpg" => io::load_png_jpeg(filename)?,
            "tiff" | "tif" => io::load_tiff(filename)?,

            _ => return Err(Error::InvalidExtension(extension)),
        };

        Ok(image)
    }

    pub fn save_file<P: AsRef<Path>>(&self, filename: P) -> Result<()> {
        let extension = filename
            .as_ref()
            .extension()
            .and_then(|os_str| os_str.to_str())
            .ok_or_else(|| Error::InvalidExtension("missing extension".to_string()))?
            .to_ascii_lowercase();

        match extension.as_str() {
            "png" => io::save_png(self, filename)?,
            "jpeg" | "jpg" => io::save_jpg(self, filename)?,
            "tiff" | "tif" => tiff::save_tiff(self, filename)?,

            _ => return Err(Error::InvalidExtension(extension)),
        };

        Ok(())
    }

    pub fn convert(self, color_format: ColorFormat) -> Result<Image> {
        color_format.validate()?;

        if self.desc.color_format == color_format {
            return Ok(self);
        }

        let desc = ImageDesc::new(self.desc.width, self.desc.height, color_format);
        let mut result = Image::new_black(desc)?;

        convert_image(&self, &mut result)?;

        Ok(result)
    }

    pub fn bytes_per_pixel(&self) -> u8 {
        self.desc.color_format.byte_count()
    }
}

/// Convert `Vec<u8>` to a 16-byte-aligned `AVec<u8>`.
///
/// Always copies: a `Vec` is allocated with align 1, so reinterpreting its buffer as an
/// `AVec<_, ConstAlign<16>>` would make the destructor free it with a mismatched `Layout`
/// alignment (UB), even when the pointer happens to already be 16-aligned.
fn vec_to_avec(bytes: Vec<u8>) -> AVec<u8, ConstAlign<ALIGNMENT>> {
    AVec::from_slice(ALIGNMENT, &bytes)
}

impl ImageDesc {
    /// Create a new (tightly packed) image descriptor.
    pub fn new(width: usize, height: usize, color_format: ColorFormat) -> Self {
        Self {
            width,
            height,
            color_format,
        }
    }

    /// Total packed byte size: `height * row_bytes`.
    pub fn size_in_bytes(&self) -> usize {
        self.height * self.row_bytes()
    }

    /// Bytes per (packed) row: `width * bytes_per_pixel`.
    pub fn row_bytes(&self) -> usize {
        self.width * self.color_format.byte_count() as usize
    }

    /// Validates the descriptor: positive dimensions, valid format.
    pub fn validate(&self) -> Result<()> {
        self.color_format.validate()?;
        if self.width == 0 || self.height == 0 {
            return Err(Error::SizeMismatch(format!(
                "image dimensions must be non-zero, got {}x{}",
                self.width, self.height
            )));
        }
        Ok(())
    }
}

impl std::fmt::Display for ImageDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{} {}", self.width, self.height, self.color_format)
    }
}
