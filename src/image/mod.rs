mod conversion;
pub(crate) mod image_data;
mod io;
mod tiff;
mod transpose;

#[cfg(test)]
mod tests;

use std::path::Path;

/// Supported image file extensions for reading and writing.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tiff", "tif"];

use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};
use crate::image::conversion::convert_image;
use crate::image::image_data::interleaved::AnyImageData;

/// Image dimensions + pixel format. Pixel data is **always tightly packed**
/// (`row_bytes == width * bytes_per_pixel`, no inter-row padding) — any row
/// alignment a GPU backend needs lives inside `GpuImage` (`src/gpu/`), never here.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub width: usize,
    pub height: usize,
    pub color_format: ColorFormat,
}

/// An image: interleaved pixel data ([`AnyImageData`], stored as a typed
/// `Buffer2<[T; N]>` per the format) plus the [`ImageDesc`] that names its
/// format and dimensions. `bytes()` reinterprets the typed buffer as `&[u8]`
/// zero-copy for the conversion / GPU / io paths; `desc` is kept in sync with
/// `data` by the constructors.
#[derive(Clone, Debug)]
pub struct Image {
    pub desc: ImageDesc,
    data: AnyImageData,
}

impl Image {
    /// The interleaved pixel bytes — a zero-copy `&[u8]` view of the typed buffer.
    pub fn bytes(&self) -> &[u8] {
        self.data.bytes()
    }

    /// Copy the pixel bytes into an owned `Vec<u8>` (the typed buffer's allocation
    /// can't be reinterpreted as a `Vec<u8>`, so this copies).
    pub fn into_bytes(self) -> Vec<u8> {
        self.data.bytes().to_vec()
    }

    /// The interleaved pixel bytes, mutable — zero-copy; writes hit the buffer.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.data.bytes_mut()
    }

    pub fn new_black(desc: ImageDesc) -> Result<Image> {
        desc.validate()?;
        let data = AnyImageData::new_zeroed(desc.color_format, desc.width, desc.height);
        Ok(Image { desc, data })
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

        let data = AnyImageData::from_bytes(desc.color_format, desc.width, desc.height, &bytes);
        Ok(Image { desc, data })
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
