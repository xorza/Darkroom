use std::ops::SubAssign;
use std::path::Path;

use imaginarium::{Buffer2, ChannelCount, ChannelType, Image};
use rayon::prelude::*;

use crate::image_ops::SAMPLES_PER_BLOCK;
use crate::image_ops::rgb::Rgb;
use crate::io::image::error::ImageError;
use crate::io::image::fits::decode as fits_decode;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    TransferProvenance,
};
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::io::image::standard::{
    FITS_EXTENSIONS, STANDARD_IMAGE_EXTENSIONS, f32_target_format, file_extension,
    read_standard_image, scientific_rejection, standard_container,
};
use crate::io::raw;
use crate::stacking::frame_store::StackableImage;

/// A one- or three-channel floating-point image in a linear numeric domain.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub metadata: ImageMetadata,
    pub(crate) pixels: LinearPixels,
    /// Which pixels carry no measurement, for a source that declared any. The samples at those
    /// positions are a finite fill, not data — see [`NullMask`].
    pub(crate) nulls: Option<NullMask>,
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
    pub fn from_file<P: AsRef<Path>>(path: P, context: &LoadContext) -> Result<Self, ImageError> {
        let path = path.as_ref();
        context.check_cancelled(path)?;
        let extension = file_extension(path);

        if FITS_EXTENSIONS.contains(&extension.as_str()) {
            return fits_decode::load_linear_fits(path, context);
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
            context.check_cancelled(path)?;
            if decoded.desc().color_format.channel_type != ChannelType::Float {
                return Err(scientific_rejection(
                    path,
                    "scientific raster input must be an explicitly declared floating-point TIFF",
                ));
            }
            if decoded.desc().color_format.channel_count == ChannelCount::Rgba {
                return Err(scientific_rejection(
                    path,
                    "scientific raster input must not contain an alpha channel",
                ));
            }

            let color = if decoded.desc().color_format.channel_count == ChannelCount::L {
                ColorProvenance::Monochrome
            } else {
                ColorProvenance::Unspecified
            };
            let mut image = LinearImage::from(&decoded);
            image.metadata.provenance = Some(ImageProvenance {
                container: standard_container(&extension),
                decoder: DecoderProvenance::Imaginarium,
                transfer: TransferProvenance::DeclaredLinearRaster,
                color,
                clipped: false,
                demosaic: DemosaicProvenance::None,
                // Every raster format this path reads stores its first row at the top.
                row_order: RowOrder::TopDown,
            });
            return Ok(image);
        }

        Err(ImageError::UnsupportedFormat { extension })
    }

    /// Create from dimensions and interleaved pixel data (RGBRGBRGB...).
    pub fn from_pixels(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        LinearImage {
            metadata: ImageMetadata::default(),
            pixels: LinearPixels::from_interleaved(dimensions, pixels),
            nulls: None,
        }
    }

    /// Create from planar channel data ([R, G, B] or single channel).
    pub fn from_planar_channels(
        dimensions: ImageDimensions,
        channels: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        LinearImage {
            metadata: ImageMetadata::default(),
            pixels: LinearPixels::from_planar_channels(dimensions, channels),
            nulls: None,
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
        matches!(self.pixels, LinearPixels::L(_))
    }

    pub fn is_rgb(&self) -> bool {
        matches!(self.pixels, LinearPixels::Rgb(_))
    }

    /// Get channel as Buffer2 reference (0=L or R, 1=G, 2=B).
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        self.pixels.channel(c)
    }

    /// Get channel as mutable Buffer2 reference.
    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        self.pixels.channel_mut(c)
    }

    /// Iterate the channel planes in channel order: one for grayscale, three for RGB.
    pub(crate) fn planes_mut(&mut self) -> impl Iterator<Item = &mut Buffer2<f32>> {
        self.pixels.planes_mut()
    }

    /// The three channel planes' samples, borrowed at once — what a cross-channel per-pixel op
    /// needs to walk R, G and B in step.
    ///
    /// # Panics
    /// On a grayscale image; callers gate on [`Self::is_rgb`].
    pub(crate) fn rgb_planes_mut(&mut self) -> [&mut [f32]; 3] {
        self.pixels.rgb_planes_mut()
    }

    /// Deinterleave an already-`f32` (`L_F32` / `RGB_F32`) imaginarium image into planes.
    ///
    /// # Panics
    /// If `image` is not one of those two formats. Callers check the format themselves; the
    /// shared gate this used to name was removed when the ops moved to `LinearImage`.
    pub(crate) fn from_f32_image(image: &Image) -> Self {
        LinearImage {
            metadata: ImageMetadata::default(),
            pixels: LinearPixels::from_f32_image(image),
            nulls: None,
        }
    }

    /// Calculate mean pixel value across all channels.
    pub fn mean(&self) -> f32 {
        self.pixels.mean()
    }

    /// Per-sample parallel in-place map over every plane. For work that treats each sample alike
    /// whatever channel it belongs to; [`Self::map_rgb`] is the form for work that needs the whole
    /// pixel.
    pub(crate) fn map_samples(&mut self, sample: impl Fn(f32) -> f32 + Sync) {
        for plane in self.planes_mut() {
            plane
                .pixels_mut()
                .par_chunks_mut(SAMPLES_PER_BLOCK)
                .for_each(|block| {
                    for value in block {
                        *value = sample(*value);
                    }
                });
        }
    }

    /// Per-pixel parallel in-place map that needs all three channels at once. A no-op on a
    /// grayscale image, which has no cross-channel relationship for `rgb` to act on — every caller
    /// (SCNR, background neutralization, the colour-preserving stretch) is meaningless in mono and
    /// returned early on it when the storage was interleaved.
    pub(crate) fn map_rgb(&mut self, rgb: impl Fn(Rgb) -> Rgb + Sync) {
        if !self.is_rgb() {
            return;
        }
        let [r, g, b] = self.rgb_planes_mut();
        r.par_chunks_mut(SAMPLES_PER_BLOCK)
            .zip(g.par_chunks_mut(SAMPLES_PER_BLOCK))
            .zip(b.par_chunks_mut(SAMPLES_PER_BLOCK))
            .for_each(|((r, g), b)| {
                for ((r, g), b) in r.iter_mut().zip(g.iter_mut()).zip(b.iter_mut()) {
                    let out = rgb(Rgb {
                        r: *r,
                        g: *g,
                        b: *b,
                    });
                    *r = out.r;
                    *g = out.g;
                    *b = out.b;
                }
            });
    }

    /// Per-pixel combined intensity as a plane: the channel itself for L, `(r+g+b)/3` for RGB.
    pub(crate) fn intensity_plane(&self) -> Buffer2<f32> {
        if !self.is_rgb() {
            return self.channel(0).clone();
        }
        let (r, g, b) = (
            self.channel(0).pixels(),
            self.channel(1).pixels(),
            self.channel(2).pixels(),
        );
        let mut intensity = vec![0.0f32; self.pixel_count()];
        intensity
            .par_iter_mut()
            .zip(r.par_iter())
            .zip(g.par_iter())
            .zip(b.par_iter())
            .for_each(|(((out, &r), &g), &b)| *out = Rgb { r, g, b }.intensity());
        Buffer2::new(self.width(), self.height(), intensity)
    }

    /// Enhance in the intensity (luminance) domain: take the combined intensity, transform it with
    /// `map`, then rescale every channel hue-preservingly so the new intensity matches. The shape
    /// shared by the display enhancers ([`crate::image_ops::hdr`],
    /// [`crate::image_ops::local_contrast`]).
    pub(crate) fn remap_intensity(&mut self, map: impl FnOnce(&Buffer2<f32>) -> Buffer2<f32>) {
        let intensity = self.intensity_plane();
        let mapped = map(&intensity);
        self.apply_intensity_remap(&intensity, &mapped);
    }

    /// Hue-preserving intensity remap: scale each pixel's channels by `mapped/intensity`
    /// (with a highlight cap so a channel can't clip past white and shift hue); L takes
    /// `mapped` directly. Output clamped to `[0, 1]`. `intensity`/`mapped` must match the
    /// image's dimensions.
    fn apply_intensity_remap(&mut self, intensity: &Buffer2<f32>, mapped: &Buffer2<f32>) {
        if !self.is_rgb() {
            self.channel_mut(0)
                .pixels_mut()
                .par_iter_mut()
                .zip(mapped.pixels().par_iter())
                .for_each(|(p, &m)| *p = m.clamp(0.0, 1.0));
            return;
        }
        let [r, g, b] = self.rgb_planes_mut();
        r.par_iter_mut()
            .zip(g.par_iter_mut())
            .zip(b.par_iter_mut())
            .zip(intensity.pixels().par_iter())
            .zip(mapped.pixels().par_iter())
            .for_each(|((((r, g), b), &i), &m)| {
                if i <= 0.0 {
                    return;
                }
                let gain = m / i;
                let (mut nr, mut ng, mut nb) = (*r * gain, *g * gain, *b * gain);
                let maxc = nr.max(ng).max(nb);
                if maxc > 1.0 {
                    let s = 1.0 / maxc;
                    nr *= s;
                    ng *= s;
                    nb *= s;
                }
                *r = nr.max(0.0);
                *g = ng.max(0.0);
                *b = nb.max(0.0);
            });
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

    fn nulls(&self) -> Option<&NullMask> {
        self.nulls.as_ref()
    }

    fn channel(&self, c: usize) -> &[f32] {
        LinearImage::channel(self, c)
    }

    fn metadata(&self) -> &ImageMetadata {
        &self.metadata
    }

    fn load(path: &Path, context: &LoadContext) -> Result<Self, ImageError> {
        LinearImage::from_file(path, context)
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
            nulls: None,
        }
    }
}

impl From<[Buffer2<f32>; 3]> for LinearImage {
    fn from(planes: [Buffer2<f32>; 3]) -> Self {
        Self {
            metadata: ImageMetadata::default(),
            pixels: planes.into(),
            nulls: None,
        }
    }
}

impl From<&LinearImage> for Image {
    fn from(linear: &LinearImage) -> Self {
        Image::from(&linear.pixels)
    }
}

/// Deinterleave an imaginarium image into planes, converting to `f32` first when it is not already.
///
/// The inbound half of the boundary `impl From<&LinearImage> for Image` forms the outbound half of:
/// what a caller holding interleaved pixels goes through to reach the ops, which take planes.
impl From<&Image> for LinearImage {
    /// Deinterleaves into planes, converting to the matching `f32` channel format first when the
    /// source is not already in one, rather than asking the caller to arrive with one.
    fn from(image: &Image) -> Self {
        let target = f32_target_format(image);
        if image.desc().color_format == target {
            Self::from_f32_image(image)
        } else {
            let converted = image
                .convert_to(target)
                .expect("image converts to its f32 channel format");
            Self::from_f32_image(&converted)
        }
    }
}

impl From<LinearImage> for Image {
    fn from(linear: LinearImage) -> Self {
        Image::from(&linear)
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::image_ops::rgb::Rgb;
    use crate::io::image::linear::LinearImage;
    use crate::io::image::linear_pixels;
    use crate::math::vec2us::Vec2us;

    impl LinearImage {
        pub(crate) fn get_pixel_gray(&self, pos: Vec2us) -> f32 {
            debug_assert!(self.is_grayscale());
            self.channel(0)[self.dimensions().size().index_of(pos)]
        }

        pub(crate) fn get_pixel_gray_mut(&mut self, pos: Vec2us) -> &mut f32 {
            debug_assert!(self.is_grayscale());
            let idx = self.dimensions().size().index_of(pos);
            &mut self.channel_mut(0)[idx]
        }

        pub(crate) fn get_pixel_channel(&self, pos: Vec2us, c: usize) -> f32 {
            debug_assert!(c < self.channels());
            self.channel(c)[self.dimensions().size().index_of(pos)]
        }

        pub(crate) fn get_pixel_rgb(&self, pos: Vec2us) -> Rgb {
            debug_assert!(self.is_rgb());
            let idx = self.dimensions().size().index_of(pos);
            Rgb {
                r: self.channel(0)[idx],
                g: self.channel(1)[idx],
                b: self.channel(2)[idx],
            }
        }

        pub(crate) fn set_pixel_rgb(&mut self, pos: Vec2us, rgb: Rgb) {
            debug_assert!(self.is_rgb());
            let idx = self.dimensions().size().index_of(pos);
            self.channel_mut(0)[idx] = rgb.r;
            self.channel_mut(1)[idx] = rgb.g;
            self.channel_mut(2)[idx] = rgb.b;
        }

        pub(crate) fn into_interleaved_pixels(self) -> Vec<f32> {
            linear_pixels::internals::into_interleaved_pixels(self.pixels)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::prelude::*;

    #[test]
    fn map_rgb_maps_every_channel_of_a_pixel_and_skips_grayscale() {
        // 2x1 RGB: pixels (0.1,0.2,0.3) and (0.4,0.5,0.6).
        let mut image = rgb_image(
            Size2us::new(2, 1),
            vec![0.1, 0.4],
            vec![0.2, 0.5],
            vec![0.3, 0.6],
        );
        image.map_rgb(|px| px.scale(2.0));
        assert_eq!(image.channel(0).pixels(), &[0.2, 0.8]);
        assert_eq!(image.channel(1).pixels(), &[0.4, 1.0]);
        assert_eq!(image.channel(2).pixels(), &[0.6, 1.2]);

        // Grayscale has no cross-channel relationship for `rgb` to act on, so it is left alone
        // rather than having the closure applied to (l, l, l).
        let mut gray = gray_image(Size2us::new(3, 1), vec![0.25, 0.5, 0.75]);
        gray.map_rgb(|px| px.scale(2.0));
        assert_eq!(gray.channel(0).pixels(), &[0.25, 0.5, 0.75]);
    }

    #[test]
    fn map_samples_maps_every_plane() {
        let mut image = rgb_image(
            Size2us::new(2, 1),
            vec![0.1, 0.4],
            vec![0.2, 0.5],
            vec![0.3, 0.6],
        );
        image.map_samples(|v| v + 1.0);
        assert_eq!(image.channel(0).pixels(), &[1.1, 1.4]);
        assert_eq!(image.channel(1).pixels(), &[1.2, 1.5]);
        assert_eq!(image.channel(2).pixels(), &[1.3, 1.6]);

        let mut gray = gray_image(Size2us::new(3, 1), vec![0.0, 0.25, 0.5]);
        gray.map_samples(|v| v + 0.25);
        assert_eq!(gray.channel(0).pixels(), &[0.25, 0.5, 0.75]);
    }

    #[test]
    fn intensity_plane_is_channel_mean_for_rgb_and_identity_for_l() {
        // RGB: (0.3,0,0) → 0.1, (0.6,0.6,0.6) → 0.6 (mean; approx for the /3 rounding).
        let rgb = rgb_image(
            Size2us::new(2, 1),
            vec![0.3, 0.6],
            vec![0.0, 0.6],
            vec![0.0, 0.6],
        );
        let i = rgb.intensity_plane();
        assert!((i.pixels()[0] - 0.1).abs() < 1e-6 && (i.pixels()[1] - 0.6).abs() < 1e-6);

        let l = gray_image(Size2us::new(2, 1), vec![0.2, 0.7]);
        assert_eq!(l.intensity_plane().pixels(), &[0.2, 0.7]);
    }

    #[test]
    fn apply_intensity_remap_scales_rgb_hue_preservingly() {
        // One pixel (0.2,0.1,0.1), I = 0.4/3; double the mapped intensity → gain 2.
        let mut image = rgb_image(Size2us::new(1, 1), vec![0.2], vec![0.1], vec![0.1]);
        let intensity = image.intensity_plane();
        let mapped = Buffer2::new(1, 1, vec![intensity.pixels()[0] * 2.0]);
        image.apply_intensity_remap(&intensity, &mapped);
        // each channel x2, none exceeds 1 → no cap
        assert_eq!(image.channel(0).pixels(), &[0.4]);
        assert_eq!(image.channel(1).pixels(), &[0.2]);
        assert_eq!(image.channel(2).pixels(), &[0.2]);
    }
}
