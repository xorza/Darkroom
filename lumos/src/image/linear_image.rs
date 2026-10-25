use std::borrow::Borrow;
use std::path::Path;
use std::{array, slice};

use common::{Buffer2, Size2us, par};
use imaginarium::{ChannelCount, ColorFormat, InterleavedImage};
use rayon::prelude::*;

use crate::image::SAMPLES_PER_BLOCK;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::intensity::Intensity;
use crate::image::linear_pixels::LinearPixels;
use crate::image::pixel_flags::PixelFlags;
use crate::image::rgb::Rgb;
use crate::image::stackable_image::{ImageParts, StackableImage};
use crate::io::error::WriteError;
use crate::io::{fits, raster};

/// A one- or three-channel floating-point image in a linear numeric domain.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub metadata: ImageMetadata,
    pixels: LinearPixels,
    /// The data-quality flags of the pixels that carry any — see [`PixelFlags`]. The samples under
    /// [`QualityFlags::NO_DATA`](crate::image::quality_flags::QualityFlags::NO_DATA) are a finite
    /// fill, not data.
    flags: Option<PixelFlags>,
    /// The gain a flat applied to each pixel's value and noise, in this image's own pixels — see
    /// [`FrameGain`]. `None` for an image no flat divided, and for a master, which the combine made.
    flat_gain: Option<FrameGain>,
}

impl LinearImage {
    /// Write the image to `path` as a checksummed floating-point FITS file: a plane or an RGB
    /// cube, with its flags and flat gain in `LUMFLAGS` and `LUMGAIN` extensions beside it;
    /// [`LoadCtx::linear`](crate::LoadCtx::linear) reads it back.
    pub fn write_fits(&self, path: impl AsRef<Path>) -> Result<(), WriteError> {
        fits::linear::save(path.as_ref(), self)
    }

    /// Write the image to `path` as the raster format its extension names: PNG, JPEG or TIFF.
    pub fn write_raster(&self, path: impl AsRef<Path>) -> Result<(), WriteError> {
        raster::save(path.as_ref(), self)
    }

    /// The image of `pixels`, its `flags` and `metadata`, with no flat gain.
    pub(crate) fn new(
        pixels: LinearPixels,
        flags: Option<PixelFlags>,
        metadata: ImageMetadata,
    ) -> Self {
        let mut image = Self {
            metadata,
            pixels,
            flags: None,
            flat_gain: None,
        };
        image.set_flags(flags);
        image
    }

    /// Create from dimensions and interleaved pixel data (RGBRGBRGB...).
    pub fn from_interleaved(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        Self::new(
            LinearPixels::from_interleaved(dimensions, pixels),
            None,
            ImageMetadata::default(),
        )
    }

    /// Create from planar channel data ([R, G, B] or single channel).
    pub fn from_planar_channels(
        dimensions: ImageDimensions,
        channels: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        Self::new(
            LinearPixels::from_planar_channels(dimensions, channels),
            None,
            ImageMetadata::default(),
        )
    }

    pub fn size(&self) -> Size2us {
        self.dimensions().size()
    }

    pub fn width(&self) -> usize {
        self.dimensions().width()
    }

    pub fn height(&self) -> usize {
        self.dimensions().height()
    }

    pub const fn channels(&self) -> usize {
        self.pixels.channel_count()
    }

    pub fn dimensions(&self) -> ImageDimensions {
        self.pixels.dimensions()
    }

    pub fn pixel_count(&self) -> usize {
        self.dimensions().pixel_count()
    }

    pub fn sample_count(&self) -> usize {
        self.dimensions().sample_count()
    }

    pub const fn is_mono(&self) -> bool {
        matches!(self.pixels, LinearPixels::Mono(_))
    }

    pub const fn is_rgb(&self) -> bool {
        matches!(self.pixels, LinearPixels::Rgb(_))
    }

    /// The interleaved format these planes repack to: `RGB_F32` or `MONO_F32`.
    pub const fn color_format(&self) -> ColorFormat {
        if self.is_rgb() {
            ColorFormat::RGB_F32
        } else {
            ColorFormat::MONO_F32
        }
    }

    /// Get channel as Buffer2 reference (0=mono or R, 1=G, 2=B).
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        self.pixels.channel(c)
    }

    /// Get channel as mutable Buffer2 reference. The metadata's quantization σ and mosaic noise
    /// are dropped: they describe the samples as they were.
    pub fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        self.samples_mut().channel_mut(c)
    }

    /// The samples, mutably, with the noise facts that described them dropped. Every mutable path
    /// to the samples comes through here, so no change to them keeps a stale noise.
    const fn samples_mut(&mut self) -> &mut LinearPixels {
        self.metadata.derived.after_edit();
        &mut self.pixels
    }

    /// Resident RAM held by this image: its `f32` planes, its flag plane, and a flat gain it holds
    /// alone. A shared gain's owner counts it. Metadata is negligible against a plane.
    pub fn ram_bytes(&self) -> usize {
        let own_gain = match &self.flat_gain {
            Some(FrameGain::Own(gain)) => gain.ram_bytes(),
            Some(FrameGain::Shared(_)) | None => 0,
        };
        self.sample_count() * size_of::<f32>()
            + self.flags.as_ref().map_or(0, |flags| flags.bytes().len())
            + own_gain
    }

    /// The data-quality flags of the pixels; `None` when no pixel carries one.
    pub const fn flags(&self) -> Option<&PixelFlags> {
        self.flags.as_ref()
    }

    /// Replace the flags with `flags`, of these pixels.
    ///
    /// # Panics
    /// When `flags` are of another size.
    pub fn set_flags(&mut self, flags: Option<PixelFlags>) {
        assert!(
            flags
                .as_ref()
                .is_none_or(|flags| flags.size() == self.dimensions().size()),
            "flags for another geometry"
        );
        self.flags = flags;
    }

    /// The flags, moved out.
    pub(crate) const fn take_flags(&mut self) -> Option<PixelFlags> {
        self.flags.take()
    }

    /// The gain a flat applied to the pixels, for an image a flat divided.
    pub const fn flat_gain(&self) -> Option<&FrameGain> {
        self.flat_gain.as_ref()
    }

    /// Replace the flat gain with `gain`, which covers these pixels and channels.
    ///
    /// # Panics
    /// When `gain` covers another size or another channel count.
    pub fn set_flat_gain(&mut self, gain: Option<FrameGain>) {
        assert!(
            gain.as_ref()
                .is_none_or(|gain| gain.covers(self.dimensions().size(), self.channels())),
            "an image's flat gain covers another image's pixels"
        );
        self.flat_gain = gain;
    }

    /// The pixels, moved out.
    pub(crate) fn into_pixels(self) -> LinearPixels {
        self.pixels
    }

    /// Iterate the channel planes in channel order: one for mono, three for RGB.
    pub(crate) fn planes_mut(&mut self) -> impl Iterator<Item = &mut Buffer2<f32>> {
        self.samples_mut().planes_mut()
    }

    /// The channel planes to edit, in channel order, beside the flags of their pixels: a split
    /// borrow, so an edit that leaves flagged pixels out reads the flags without a copy of them.
    pub(crate) fn planes_mut_with_flags(&mut self) -> PlanesWithFlags<'_> {
        self.metadata.derived.after_edit();
        PlanesWithFlags {
            planes: self.pixels.planes_mut(),
            flags: self.flags.as_ref(),
        }
    }

    /// The three channel planes' samples, borrowed at once — what a cross-channel per-pixel op
    /// needs to walk R, G and B in step.
    ///
    /// # Panics
    /// On a mono image; callers gate on [`Self::is_rgb`].
    pub(crate) fn rgb_planes_mut(&mut self) -> [&mut [f32]; 3] {
        self.samples_mut().rgb_planes_mut()
    }

    /// Deinterleave an already-`f32` (`MONO_F32` / `RGB_F32`) imaginarium image into planes.
    ///
    /// # Panics
    /// If `image` is not one of those two formats. Callers check the format themselves — the
    /// ops live on `LinearImage`, so there is no shared gate to name.
    fn from_f32_image<I>(image: I) -> Self
    where
        I: Borrow<InterleavedImage>,
        [Buffer2<f32>; 1]: TryFrom<I, Error = imaginarium::Error>,
        [Buffer2<f32>; 3]: TryFrom<I, Error = imaginarium::Error>,
    {
        Self::new(
            LinearPixels::from_f32_image(image),
            None,
            ImageMetadata::default(),
        )
    }

    /// Per-pixel parallel in-place map that needs all three channels at once. A no-op on a
    /// mono image, which has no cross-channel relationship for `rgb` to act on — every caller
    /// (SCNR, background neutralization, the color-preserving stretch) is meaningless in mono and
    /// returned early on it when the storage was interleaved.
    pub(crate) fn map_rgb(&mut self, rgb: impl Fn(Rgb) -> Rgb + Sync) {
        if !self.is_rgb() {
            return;
        }
        let [r, g, b] = self.rgb_planes_mut();
        par::chunks_mut(r, SAMPLES_PER_BLOCK)
            .zip(par::chunks_mut(g, SAMPLES_PER_BLOCK))
            .zip(par::chunks_mut(b, SAMPLES_PER_BLOCK))
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

    /// The per-pixel combined intensity, read from the channels where it is needed: the channel
    /// itself for mono, `(r+g+b)/3` for RGB.
    pub(crate) fn intensity(&self) -> Intensity<'_> {
        let size = self.dimensions().size();
        if self.is_rgb() {
            Intensity::rgb(size, array::from_fn(|c| self.channel(c).pixels()))
        } else {
            Intensity::mono(size, self.channel(0).pixels())
        }
    }

    /// Enhance in the intensity (luminance) domain: take the combined intensity, transform it with
    /// `map`, then move every pixel to its new intensity with its hue kept
    /// ([`Rgb::with_intensity`]); mono takes the new intensity clamped to `[0, 1]`. The shape
    /// shared by the display enhancers (`image_ops::hdr`, `image_ops::local_contrast`).
    pub(crate) fn remap_intensity(&mut self, map: impl FnOnce(Intensity<'_>) -> Buffer2<f32>) {
        let mapped = map(self.intensity());
        if !self.is_rgb() {
            par::chunks_mut(self.channel_mut(0).pixels_mut(), SAMPLES_PER_BLOCK)
                .zip(par::chunks(mapped.pixels(), SAMPLES_PER_BLOCK))
                .for_each(|(out, mapped)| {
                    for (out, &m) in out.iter_mut().zip(mapped) {
                        *out = m.clamp(0.0, 1.0);
                    }
                });
            return;
        }
        let [r, g, b] = self.rgb_planes_mut();
        par::chunks_mut(r, SAMPLES_PER_BLOCK)
            .zip(par::chunks_mut(g, SAMPLES_PER_BLOCK))
            .zip(par::chunks_mut(b, SAMPLES_PER_BLOCK))
            .zip(par::chunks(mapped.pixels(), SAMPLES_PER_BLOCK))
            .for_each(|(((r, g), b), mapped)| {
                for (((r, g), b), &m) in
                    r.iter_mut().zip(g.iter_mut()).zip(b.iter_mut()).zip(mapped)
                {
                    let out = Rgb {
                        r: *r,
                        g: *g,
                        b: *b,
                    }
                    .with_intensity(m);
                    *r = out.r;
                    *g = out.g;
                    *b = out.b;
                }
            });
    }
}

/// [`LinearImage::planes_mut_with_flags`]: the planes, and the flags beside them.
#[derive(Debug)]
pub(crate) struct PlanesWithFlags<'a> {
    pub(crate) planes: slice::IterMut<'a, Buffer2<f32>>,
    pub(crate) flags: Option<&'a PixelFlags>,
}

impl StackableImage for LinearImage {
    fn dimensions(&self) -> ImageDimensions {
        self.dimensions()
    }

    fn flags(&self) -> Option<&PixelFlags> {
        self.flags.as_ref()
    }

    fn channel(&self, c: usize) -> &Buffer2<f32> {
        Self::channel(self, c)
    }

    fn metadata(&self) -> &ImageMetadata {
        &self.metadata
    }

    fn cfa_type(&self) -> Option<CfaType> {
        None
    }

    fn into_parts(self) -> ImageParts {
        ImageParts {
            planes: self.pixels.into_planes(),
            flags: self.flags,
            flat_gain: self.flat_gain,
        }
    }

    fn from_parts(parts: ImageParts, _cfa_type: Option<CfaType>, metadata: ImageMetadata) -> Self {
        let ImageParts {
            planes,
            flags,
            flat_gain,
        } = parts;
        let mut image = Self::new(LinearPixels::from(planes), flags, metadata);
        image.set_flat_gain(flat_gain);
        image
    }

    fn flat_gain(&self) -> Option<&FrameGain> {
        Self::flat_gain(self)
    }
}

impl From<Buffer2<f32>> for LinearImage {
    fn from(plane: Buffer2<f32>) -> Self {
        Self::new(plane.into(), None, ImageMetadata::default())
    }
}

impl From<[Buffer2<f32>; 3]> for LinearImage {
    fn from(planes: [Buffer2<f32>; 3]) -> Self {
        Self::new(planes.into(), None, ImageMetadata::default())
    }
}

impl From<&LinearImage> for InterleavedImage {
    fn from(linear: &LinearImage) -> Self {
        Self::from(&linear.pixels)
    }
}

/// Deinterleave an imaginarium image into planes, converting to `f32` first when it is not already.
///
/// The inbound half of the boundary whose outbound half is `impl From<&LinearImage> for
/// InterleavedImage`: what a caller holding interleaved pixels goes through to reach the ops, which
/// take planes.
impl From<&InterleavedImage> for LinearImage {
    /// Deinterleaves into planes, converting to the matching `f32` channel format first when the
    /// source is not already in one, rather than asking the caller to arrive with one.
    fn from(image: &InterleavedImage) -> Self {
        let target = f32_target_format(image.desc().color_format);
        if image.desc().color_format == target {
            Self::from_f32_image(image)
        } else {
            Self::from_f32_image(image.convert_to(target))
        }
    }
}

impl From<InterleavedImage> for LinearImage {
    /// The borrowing conversion, taking the image: a mono `f32` image keeps its allocation as
    /// its plane.
    fn from(image: InterleavedImage) -> Self {
        let target = f32_target_format(image.desc().color_format);
        Self::from_f32_image(image.convert(target))
    }
}

impl From<LinearImage> for InterleavedImage {
    fn from(linear: LinearImage) -> Self {
        Self::from(&linear)
    }
}

/// The `f32` format an image of `format` deinterleaves into: `MONO_F32` for mono, `RGB_F32` for
/// color, an alpha channel dropped.
pub(crate) const fn f32_target_format(format: ColorFormat) -> ColorFormat {
    match format.channel_count {
        ChannelCount::Mono => ColorFormat::MONO_F32,
        ChannelCount::Rgb | ChannelCount::Rgba => ColorFormat::RGB_F32,
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::Buffer2;

    use crate::image::linear_image::LinearImage;

    impl LinearImage {
        /// The combined intensity as a plane, for a test to measure.
        pub(crate) fn intensity_plane(&self) -> Buffer2<f32> {
            let intensity = self.intensity();
            let size = intensity.size();
            Buffer2::new(
                size,
                (0..size.pixel_count())
                    .map(|idx| intensity.at(idx))
                    .collect(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrayvec::ArrayVec;
    use common::Buffer2;

    use crate::image::flat_gain::FlatGain;
    use crate::image::flat_gain::frame_gain::FrameGain;
    use crate::image::pixel_flags::PixelFlags;
    use crate::internals::images::{mono_image, rgb_image};
    use common::Size2us;

    /// A 2×1 RGB image holds 6 `f32` samples, 24 bytes; its flag plane adds a byte a pixel, 2; a
    /// gain it holds alone adds its grid, 2 nodes × 3 channels × 4 bytes = 24; a shared one adds
    /// nothing, as its owner counts it.
    #[test]
    fn ram_bytes_counts_the_planes_the_flags_and_an_own_gain() {
        let size = Size2us::new(2, 1);
        let mut image = rgb_image(size, [vec![0.1, 0.4], vec![0.2, 0.5], vec![0.3, 0.6]]);
        assert_eq!(image.ram_bytes(), 24);
        image.set_flags(PixelFlags::from_bytes(Buffer2::new((2, 1), vec![1, 0])));
        assert_eq!(image.ram_bytes(), 26);
        let gain = FlatGain::from_nodes(
            size,
            (0..3)
                .map(|_| Buffer2::new((2, 1), vec![1.5, 2.0]))
                .collect::<ArrayVec<_, 3>>(),
        );
        image.set_flat_gain(Some(FrameGain::Own(Box::new(gain.clone()))));
        assert_eq!(image.ram_bytes(), 50);
        image.set_flat_gain(Some(FrameGain::Shared(Arc::new(gain))));
        assert_eq!(image.ram_bytes(), 26);
    }

    #[test]
    fn map_rgb_maps_every_channel_of_a_pixel_and_skips_mono() {
        // 2x1 RGB: pixels (0.1,0.2,0.3) and (0.4,0.5,0.6).
        let mut image = rgb_image(
            Size2us::new(2, 1),
            [vec![0.1, 0.4], vec![0.2, 0.5], vec![0.3, 0.6]],
        );
        image.map_rgb(|px| px.scale(2.0));
        assert_eq!(image.channel(0).pixels(), &[0.2, 0.8]);
        assert_eq!(image.channel(1).pixels(), &[0.4, 1.0]);
        assert_eq!(image.channel(2).pixels(), &[0.6, 1.2]);

        // Mono has no cross-channel relationship for `rgb` to act on, so it is left alone
        // rather than having the closure applied to (l, l, l).
        let mut mono = mono_image(Size2us::new(3, 1), vec![0.25, 0.5, 0.75]);
        mono.map_rgb(|px| px.scale(2.0));
        assert_eq!(mono.channel(0).pixels(), &[0.25, 0.5, 0.75]);
    }

    /// The channel mean as f32 rounds it, the sum times the f32 `1/3`: (0.3, 0, 0) gives
    /// `0.3·⅓`, which rounds one ulp above `0.1f32`, to `0x3dcc_ccce`; (0.6, 0.6, 0.6) sums to
    /// `1.8000001` and its third rounds back onto `0.6f32`. Mono is its own intensity.
    #[test]
    fn intensity_plane_is_channel_mean_for_rgb_and_identity_for_mono() {
        let rgb = rgb_image(
            Size2us::new(2, 1),
            [vec![0.3, 0.6], vec![0.0, 0.6], vec![0.0, 0.6]],
        );
        assert_eq!(
            rgb.intensity_plane().pixels(),
            &[f32::from_bits(0x3dcc_ccce), 0.6]
        );

        let l = mono_image(Size2us::new(2, 1), vec![0.2, 0.7]);
        assert_eq!(l.intensity_plane().pixels(), &[0.2, 0.7]);
    }

    /// Doubling the intensity doubles every channel, `2I / I` being 2 exactly; a mono plane takes
    /// the mapped intensity itself, clamped to `[0, 1]`.
    #[test]
    fn remap_intensity_moves_pixels_with_their_hue() {
        let mut image = rgb_image(Size2us::new(1, 1), [vec![0.2], vec![0.1], vec![0.1]]);
        image.remap_intensity(|intensity| Buffer2::new((1, 1), vec![2.0 * intensity.at(0)]));
        assert_eq!(image.channel(0).pixels(), &[0.4]);
        assert_eq!(image.channel(1).pixels(), &[0.2]);
        assert_eq!(image.channel(2).pixels(), &[0.2]);

        let mut mono = mono_image(Size2us::new(3, 1), vec![0.2, 0.5, 0.7]);
        mono.remap_intensity(|_| Buffer2::new((3, 1), vec![-0.5, 0.25, 1.5]));
        assert_eq!(mono.channel(0).pixels(), &[0.0, 0.25, 1.0]);
    }
}
