use std::ops::SubAssign;
use std::path::Path;

use imaginarium::{Buffer2, ChannelCount, ChannelType, DeinterleavedImageData, Image};
use rayon::prelude::*;

use crate::io::image::error::ImageError;
use crate::io::image::fits;
use crate::io::image::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, FITS_EXTENSIONS, ImageDimensions,
    ImageMetadata, ImageProvenance, STANDARD_IMAGE_EXTENSIONS, TransferProvenance,
    f32_target_format, file_extension, read_standard_image, scientific_rejection,
    standard_container,
};
use crate::io::raw;
use crate::math::sum::sum_f32;
use crate::stacking::frame_store::StackableImage;

/// Pixel data storage - planar format for efficient per-channel operations.
#[derive(Debug, Clone)]
pub(crate) enum PixelData {
    L(DeinterleavedImageData<1, f32>),
    Rgb(DeinterleavedImageData<3, f32>),
}

impl PixelData {
    pub(crate) fn new_default(width: usize, height: usize, channels: usize) -> Self {
        match channels {
            1 => PixelData::L(DeinterleavedImageData::new_zeroed(width, height)),
            3 => PixelData::Rgb(DeinterleavedImageData::new_zeroed(width, height)),
            _ => panic!("Only 1 or 3 channels supported, got {channels}"),
        }
    }

    pub(crate) fn channel(&self, c: usize) -> &Buffer2<f32> {
        match self {
            PixelData::L(img) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {c}");
                &img.channels[0]
            }
            PixelData::Rgb(img) => {
                assert!(c < 3, "RGB image has channels 0-2, got {c}");
                &img.channels[c]
            }
        }
    }

    pub(crate) fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        match self {
            PixelData::L(img) => {
                assert!(c == 0, "Grayscale image only has channel 0, got {c}");
                &mut img.channels[0]
            }
            PixelData::Rgb(img) => {
                assert!(c < 3, "RGB image has channels 0-2, got {c}");
                &mut img.channels[c]
            }
        }
    }

    pub(crate) fn channels(&self) -> usize {
        match self {
            PixelData::L(_) => 1,
            PixelData::Rgb(_) => 3,
        }
    }

    pub(crate) fn into_l(self) -> Buffer2<f32> {
        match self {
            PixelData::L(img) => {
                let [data] = img.channels;
                data
            }
            PixelData::Rgb(_) => panic!("Expected L variant, got Rgb"),
        }
    }
}

/// A one- or three-channel floating-point image in a linear numeric domain.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
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
        assert_eq!(
            pixels.len(),
            dimensions.sample_count(),
            "Sample count mismatch: expected {}, got {}",
            dimensions.sample_count(),
            pixels.len()
        );

        let width = dimensions.size.x;
        let height = dimensions.size.y;
        let pixel_data = if dimensions.is_grayscale() {
            PixelData::L(DeinterleavedImageData::from_channels([Buffer2::new(
                width, height, pixels,
            )]))
        } else {
            let mut r: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut g: Buffer2<f32> = Buffer2::new_default(width, height);
            let mut b: Buffer2<f32> = Buffer2::new_default(width, height);
            deinterleave_rgb(&pixels, r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
            PixelData::Rgb(DeinterleavedImageData::from_channels([r, g, b]))
        };

        LinearImage {
            metadata: ImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    /// Create from planar channel data ([R, G, B] or single channel).
    pub fn from_planar_channels(
        dimensions: ImageDimensions,
        channels: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        let expected_pixels_per_channel = dimensions.size.x * dimensions.size.y;
        let width = dimensions.size.x;
        let height = dimensions.size.y;

        let mut iter = channels.into_iter();
        let pixel_data = if dimensions.is_grayscale() {
            let ch = iter.next().expect("Expected 1 channel for grayscale");
            assert_eq!(
                ch.len(),
                expected_pixels_per_channel,
                "Channel 0 pixel count mismatch"
            );
            assert!(iter.next().is_none(), "Too many channels for grayscale");
            PixelData::L(DeinterleavedImageData::from_channels([Buffer2::new(
                width, height, ch,
            )]))
        } else {
            let r = iter.next().expect("Expected 3 channels for RGB");
            let g = iter.next().expect("Expected 3 channels for RGB");
            let b = iter.next().expect("Expected 3 channels for RGB");
            assert_eq!(
                r.len(),
                expected_pixels_per_channel,
                "R channel pixel count mismatch"
            );
            assert_eq!(
                g.len(),
                expected_pixels_per_channel,
                "G channel pixel count mismatch"
            );
            assert_eq!(
                b.len(),
                expected_pixels_per_channel,
                "B channel pixel count mismatch"
            );
            assert!(iter.next().is_none(), "Too many channels for RGB");
            PixelData::Rgb(DeinterleavedImageData::from_channels([
                Buffer2::new(width, height, r),
                Buffer2::new(width, height, g),
                Buffer2::new(width, height, b),
            ]))
        };

        LinearImage {
            metadata: ImageMetadata::default(),
            dimensions,
            pixels: pixel_data,
        }
    }

    pub fn width(&self) -> usize {
        self.dimensions.size.x
    }

    pub fn height(&self) -> usize {
        self.dimensions.size.y
    }

    pub fn channels(&self) -> usize {
        self.dimensions.channels
    }

    pub fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    pub fn pixel_count(&self) -> usize {
        self.dimensions.pixel_count()
    }

    pub fn sample_count(&self) -> usize {
        self.dimensions.sample_count()
    }

    pub fn is_grayscale(&self) -> bool {
        self.dimensions.is_grayscale()
    }

    pub fn is_rgb(&self) -> bool {
        self.dimensions.is_rgb()
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
        fn parallel_sum(values: &[f32]) -> f32 {
            values.par_chunks(8192).map(sum_f32).sum()
        }

        match &self.pixels {
            PixelData::L(img) => {
                let data = &img.channels[0];
                debug_assert!(!data.is_empty());
                parallel_sum(data) / data.len() as f32
            }
            PixelData::Rgb(img) => {
                let [r, g, b] = &img.channels;
                let total = parallel_sum(r) + parallel_sum(g) + parallel_sum(b);
                let count = r.len() + g.len() + b.len();
                total / count as f32
            }
        }
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
        let mut planes = arrayvec::ArrayVec::new();
        match self.pixels {
            PixelData::L(img) => {
                let [buffer] = img.channels;
                planes.push(buffer);
            }
            PixelData::Rgb(img) => planes.extend(img.channels),
        }
        planes
    }
}

impl SubAssign<&LinearImage> for LinearImage {
    fn sub_assign(&mut self, rhs: &LinearImage) {
        assert_eq!(self.dimensions, rhs.dimensions, "Image dimensions mismatch");
        let w = self.dimensions.size.x;
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
        let dimensions = ImageDimensions::new((plane.width(), plane.height()), 1);
        Self {
            metadata: ImageMetadata::default(),
            dimensions,
            pixels: PixelData::L(DeinterleavedImageData::from_channels([plane])),
        }
    }
}

impl From<[Buffer2<f32>; 3]> for LinearImage {
    fn from(planes: [Buffer2<f32>; 3]) -> Self {
        let dimensions = ImageDimensions::new((planes[0].width(), planes[0].height()), 3);
        Self {
            metadata: ImageMetadata::default(),
            dimensions,
            pixels: PixelData::Rgb(DeinterleavedImageData::from_channels(planes)),
        }
    }
}

impl From<&LinearImage> for Image {
    fn from(linear: &LinearImage) -> Self {
        // imaginarium owns the planar→interleaved transpose; each `PixelData`
        // arm already holds the `DeinterleavedImageData` it interleaves from
        // (borrowed, so an RGB master isn't cloned — `save` takes `&self`).
        match &linear.pixels {
            PixelData::L(planes) => Image::from(planes),
            PixelData::Rgb(planes) => Image::from(planes),
        }
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
    let (width, height) = (image.desc.width, image.desc.height);
    // imaginarium owns the interleaved→planar transpose; the image is guaranteed
    // f32 here, so the variant deinterleaves into 1 or 3 planes.
    let pixels = match image.desc.color_format.channel_count {
        ChannelCount::L => PixelData::L(
            image
                .try_into()
                .expect("L_F32 image deinterleaves to 1 plane"),
        ),
        _ => PixelData::Rgb(
            image
                .try_into()
                .expect("RGB_F32 image deinterleaves to 3 planes"),
        ),
    };

    let dimensions = ImageDimensions::new((width, height), pixels.channels());
    LinearImage {
        metadata: ImageMetadata::default(),
        dimensions,
        pixels,
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

/// Deinterleave RGB data (RGBRGB...) into separate R, G, B planes.
fn deinterleave_rgb(interleaved: &[f32], r: &mut [f32], g: &mut [f32], b: &mut [f32]) {
    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    r.par_iter_mut()
        .zip(g.par_iter_mut())
        .zip(b.par_iter_mut())
        .enumerate()
        .for_each(|(i, ((r_val, g_val), b_val))| {
            let src_idx = i * 3;
            *r_val = interleaved[src_idx];
            *g_val = interleaved[src_idx + 1];
            *b_val = interleaved[src_idx + 2];
        });
}

#[cfg(test)]
fn interleave_rgb(r: &[f32], g: &[f32], b: &[f32], interleaved: &mut [f32]) {
    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    interleaved
        .par_chunks_mut(3)
        .enumerate()
        .for_each(|(i, rgb)| {
            rgb[0] = r[i];
            rgb[1] = g[i];
            rgb[2] = b[i];
        });
}

#[cfg(test)]
pub(crate) mod test_support {
    use imaginarium::Image;

    use crate::image_ops::rgb::Rgb;
    use crate::io::image::linear::{self, LinearImage, PixelData};

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
            match &self.pixels {
                PixelData::Rgb(img) => {
                    let [r, g, b] = &img.channels;
                    Rgb {
                        r: r[idx],
                        g: g[idx],
                        b: b[idx],
                    }
                }
                _ => unreachable!(),
            }
        }

        pub(crate) fn set_pixel_rgb(&mut self, x: usize, y: usize, rgb: Rgb) {
            debug_assert!(x < self.width() && y < self.height());
            debug_assert!(self.is_rgb());
            let idx = y * self.width() + x;
            match &mut self.pixels {
                PixelData::Rgb(img) => {
                    let [r, g, b] = &mut img.channels;
                    r[idx] = rgb.r;
                    g[idx] = rgb.g;
                    b[idx] = rgb.b;
                }
                _ => unreachable!(),
            }
        }

        pub(crate) fn into_interleaved_pixels(self) -> Vec<f32> {
            match self.pixels {
                PixelData::L(img) => {
                    let [data] = img.channels;
                    data.into_vec()
                }
                PixelData::Rgb(img) => {
                    let [r, g, b] = img.channels;
                    let mut interleaved = vec![0.0f32; r.len() * 3];
                    linear::interleave_rgb(&r, &g, &b, &mut interleaved);
                    interleaved
                }
            }
        }
    }
}
