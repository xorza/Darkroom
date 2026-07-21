use std::ops::SubAssign;
use std::path::Path;

use imaginarium::{Buffer2, ChannelCount, ChannelType, Image};
use rayon::prelude::*;

use crate::io::image::error::ImageError;
use crate::io::image::fits;
use crate::io::image::pixel_data::PixelData;
use crate::io::image::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, FITS_EXTENSIONS, ImageDimensions,
    ImageMetadata, ImageProvenance, STANDARD_IMAGE_EXTENSIONS, TransferProvenance,
    f32_target_format, file_extension, read_standard_image, scientific_rejection,
    standard_container,
};
use crate::io::raw;
use crate::stacking::frame_store::StackableImage;

/// A one- or three-channel floating-point image in a linear numeric domain.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub metadata: ImageMetadata,
    pub(crate) pixels: PixelData,
}

impl LinearImage {
    /// Load an already-linear scientific image from a file.
    ///
    /// Supported formats:
    /// - FITS without CFA metadata: .fit, .fits
    /// - Explicitly declared linear, floating-point TIFF: .tiff, .tif
    ///
    /// Camera RAW, mosaic FITS, integer TIFF, alpha TIFF, PNG, and JPEG are rejected. Use
    /// [`crate::CfaImage::from_file`] or [`crate::PreviewImage::from_file`] for those products.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ImageError> {
        let path = path.as_ref();
        let extension = file_extension(path);

        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            return fits::load_linear_fits(path);
        }

        if raw::RAW_EXTENSIONS.contains(&extension.as_str()) {
            return Err(scientific_rejection(
                path,
                "camera RAW must be loaded as CfaImage and calibrated before demosaicing",
            ));
        }

        if STANDARD_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            if !matches!(extension.as_str(), "tiff" | "tif") {
                return Err(scientific_rejection(
                    path,
                    "PNG and JPEG are preview-only because their transfer and color transforms are not decoded",
                ));
            }

            let decoded = read_standard_image(path)?;
            if decoded.desc.color_format.channel_type != ChannelType::Float {
                return Err(scientific_rejection(
                    path,
                    "scientific raster input must be an explicitly declared floating-point TIFF",
                ));
            }
            if decoded.desc.color_format.channel_count == ChannelCount::Rgba {
                return Err(scientific_rejection(
                    path,
                    "scientific raster input must not contain an alpha channel",
                ));
            }

            let color = if decoded.desc.color_format.channel_count == ChannelCount::L {
                ColorProvenance::Monochrome
            } else {
                ColorProvenance::Unspecified
            };
            let mut image = linear_from_image(&decoded);
            image.metadata.provenance = Some(ImageProvenance {
                container: standard_container(&extension),
                decoder: DecoderProvenance::Imaginarium,
                transfer: TransferProvenance::DeclaredLinearRaster,
                color,
                clipped: false,
                demosaic: DemosaicProvenance::None,
            });
            return Ok(image);
        }

        Err(ImageError::UnsupportedFormat { extension })
    }

    /// Create from dimensions and interleaved pixel data (RGBRGBRGB...).
    pub fn from_pixels(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        LinearImage {
            metadata: ImageMetadata::default(),
            pixels: PixelData::from_interleaved(dimensions, pixels),
        }
    }

    /// Create from planar channel data ([R, G, B] or single channel).
    pub fn from_planar_channels(
        dimensions: ImageDimensions,
        channels: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        LinearImage {
            metadata: ImageMetadata::default(),
            pixels: PixelData::from_planar_channels(dimensions, channels),
        }
    }

    pub fn width(&self) -> usize {
        self.pixels.channel(0).width()
    }

    pub fn height(&self) -> usize {
        self.pixels.channel(0).height()
    }

    pub fn channels(&self) -> usize {
        self.pixels.channel_count()
    }

    pub fn dimensions(&self) -> ImageDimensions {
        self.pixels.dimensions()
    }

    pub fn pixel_count(&self) -> usize {
        self.width() * self.height()
    }

    pub fn sample_count(&self) -> usize {
        self.pixel_count() * self.channels()
    }

    pub fn is_grayscale(&self) -> bool {
        matches!(self.pixels, PixelData::L(_))
    }

    pub fn is_rgb(&self) -> bool {
        matches!(self.pixels, PixelData::Rgb(_))
    }

    /// Get channel as Buffer2 reference (0=L or R, 1=G, 2=B).
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        self.pixels.channel(c)
    }

    /// Get channel as mutable Buffer2 reference.
    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        self.pixels.channel_mut(c)
    }

    /// Calculate mean pixel value across all channels.
    pub fn mean(&self) -> f32 {
        self.pixels.mean()
    }

    /// Save to file (PNG, JPEG, TIFF supported).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ImageError> {
        let image: Image = self.into();
        image
            .save_file(path)
            .map_err(|source| ImageError::Save { source })
    }
}

impl StackableImage for LinearImage {
    fn dimensions(&self) -> ImageDimensions {
        self.dimensions()
    }

    fn channel(&self, c: usize) -> &[f32] {
        LinearImage::channel(self, c)
    }

    fn metadata(&self) -> &ImageMetadata {
        &self.metadata
    }

    fn load(path: &Path) -> Result<Self, ImageError> {
        LinearImage::from_file(path)
    }

    fn into_planes(self) -> arrayvec::ArrayVec<Buffer2<f32>, 3> {
        self.pixels.into_planes()
    }
}

impl SubAssign<&LinearImage> for LinearImage {
    fn sub_assign(&mut self, rhs: &LinearImage) {
        assert_eq!(
            self.dimensions(),
            rhs.dimensions(),
            "Image dimensions mismatch"
        );
        let w = self.width();
        for c in 0..self.channels() {
            let dst = self.channel_mut(c).pixels_mut();
            let src = rhs.channel(c).pixels();
            dst.par_chunks_mut(w)
                .zip(src.par_chunks(w))
                .for_each(|(d_row, s_row)| {
                    for (d, s) in d_row.iter_mut().zip(s_row.iter()) {
                        *d -= s;
                    }
                });
        }
    }
}

impl From<Buffer2<f32>> for LinearImage {
    fn from(plane: Buffer2<f32>) -> Self {
        Self {
            metadata: ImageMetadata::default(),
            pixels: plane.into(),
        }
    }
}

impl From<[Buffer2<f32>; 3]> for LinearImage {
    fn from(planes: [Buffer2<f32>; 3]) -> Self {
        Self {
            metadata: ImageMetadata::default(),
            pixels: planes.into(),
        }
    }
}

impl From<&LinearImage> for Image {
    fn from(linear: &LinearImage) -> Self {
        Image::from(&linear.pixels)
    }
}

impl From<LinearImage> for Image {
    fn from(linear: LinearImage) -> Self {
        Image::from(&linear)
    }
}

/// Deinterleave an already-`f32` (`L_F32` / `RGB_F32`) imaginarium image into a
/// planar [`LinearImage`]. This is the single unavoidable copy a per-channel op
/// pays to get planar data; callers must convert to f32 first.
fn linear_from_f32_image(image: &Image) -> LinearImage {
    LinearImage {
        metadata: ImageMetadata::default(),
        pixels: PixelData::from_f32_image(image),
    }
}

fn linear_from_image(image: &Image) -> LinearImage {
    let target = f32_target_format(image);
    if image.desc.color_format == target {
        linear_from_f32_image(image)
    } else {
        let converted = image
            .convert_to(target)
            .expect("image converts to its f32 channel format");
        linear_from_f32_image(&converted)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use imaginarium::Image;

    use crate::image_ops::rgb::Rgb;
    use crate::io::image::linear::{self, LinearImage};
    use crate::io::image::pixel_data;

    pub(crate) fn from_image(image: &Image) -> LinearImage {
        linear::linear_from_image(image)
    }

    impl LinearImage {
        pub(crate) fn get_pixel_gray(&self, x: usize, y: usize) -> f32 {
            debug_assert!(x < self.width() && y < self.height());
            debug_assert!(self.is_grayscale());
            self.channel(0)[y * self.width() + x]
        }

        pub(crate) fn get_pixel_gray_mut(&mut self, x: usize, y: usize) -> &mut f32 {
            debug_assert!(x < self.width() && y < self.height());
            debug_assert!(self.is_grayscale());
            let width = self.width();
            &mut self.channel_mut(0)[y * width + x]
        }

        pub(crate) fn get_pixel_channel(&self, x: usize, y: usize, c: usize) -> f32 {
            debug_assert!(x < self.width() && y < self.height() && c < self.channels());
            self.channel(c)[y * self.width() + x]
        }

        pub(crate) fn get_pixel_rgb(&self, x: usize, y: usize) -> Rgb {
            debug_assert!(x < self.width() && y < self.height());
            debug_assert!(self.is_rgb());
            let idx = y * self.width() + x;
            Rgb {
                r: self.channel(0)[idx],
                g: self.channel(1)[idx],
                b: self.channel(2)[idx],
            }
        }

        pub(crate) fn set_pixel_rgb(&mut self, x: usize, y: usize, rgb: Rgb) {
            debug_assert!(x < self.width() && y < self.height());
            debug_assert!(self.is_rgb());
            let idx = y * self.width() + x;
            self.channel_mut(0)[idx] = rgb.r;
            self.channel_mut(1)[idx] = rgb.g;
            self.channel_mut(2)[idx] = rgb.b;
        }

        pub(crate) fn into_interleaved_pixels(self) -> Vec<f32> {
            pixel_data::test_support::into_interleaved_pixels(self.pixels)
        }
    }
}
