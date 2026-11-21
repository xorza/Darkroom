use std::path::Path;
use std::{array, slice};

use arrayvec::ArrayVec;
use common::{Buffer2, par};
use imaginarium::{InterleavedImage, PlanarDesc, PlanarImage};
use rayon::prelude::*;

use crate::image::SAMPLES_PER_BLOCK;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::frame::{Frame, sealed};
use crate::image::intensity::Intensity;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::rgb::Rgb;
use crate::io::error::WriteError;
use crate::io::{fits, raster};

/// A one- or three-channel floating-point image in a linear numeric domain.
pub type LinearImage = Frame<PlanarImage>;

impl sealed::Planes for PlanarImage {
    type Pattern = ();

    fn desc(&self) -> PlanarDesc {
        Self::desc(self)
    }

    fn channel(&self, channel: usize) -> &Buffer2<f32> {
        Self::channel(self, channel)
    }

    fn pattern(&self) {}

    fn cfa_type(&self) -> Option<CfaType> {
        None
    }

    fn pattern_of(cfa_type: Option<CfaType>) -> Option<()> {
        cfa_type.is_none().then_some(())
    }

    fn gain_channels(&self) -> usize {
        self.channel_count()
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3> {
        if self.is_mono() {
            ArrayVec::from_iter([self.into_mono()])
        } else {
            ArrayVec::from(self.into_rgb())
        }
    }

    fn from_planes(planes: ArrayVec<Buffer2<f32>, 3>, (): ()) -> Self {
        match planes.into_inner() {
            Ok(rgb) => Self::new_rgb(rgb),
            Err(mut mono) => {
                let plane = mono.pop().expect("an image has one plane or three");
                assert!(mono.is_empty(), "an image has one plane or three");
                Self::new_mono(plane)
            }
        }
    }
}

impl Frame<PlanarImage> {
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
    ///
    /// # Panics
    /// When `pixels` have a side past [`MAX_SIDE`](crate::frame_desc::MAX_SIDE).
    pub(crate) fn new(
        pixels: PlanarImage,
        flags: Option<PixelFlags>,
        metadata: ImageMetadata,
    ) -> Self {
        let mut image = Self::of(pixels, metadata);
        image.set_flags(flags);
        image
    }

    /// The image of `desc` over interleaved samples (RGBRGB…), with default metadata: see
    /// [`PlanarImage::from_interleaved`].
    pub fn from_interleaved(desc: PlanarDesc, samples: Vec<f32>) -> Self {
        Self::from(PlanarImage::from_interleaved(desc, samples))
    }

    /// The image of `desc` over one plane per channel, with default metadata: see
    /// [`PlanarImage::from_planar_channels`].
    pub fn from_planar_channels(
        desc: PlanarDesc,
        planes: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        Self::from(PlanarImage::from_planar_channels(desc, planes))
    }

    pub fn width(&self) -> usize {
        self.desc().width()
    }

    pub fn height(&self) -> usize {
        self.desc().height()
    }

    pub const fn channel_count(&self) -> usize {
        self.samples.channel_count()
    }

    pub fn pixel_count(&self) -> usize {
        self.desc().pixel_count()
    }

    pub fn sample_count(&self) -> usize {
        self.desc().sample_count()
    }

    pub const fn is_mono(&self) -> bool {
        self.samples.is_mono()
    }

    pub const fn is_rgb(&self) -> bool {
        self.samples.is_rgb()
    }

    /// The plane of channel `c`, mutably. The metadata's quantization σ and mosaic noise are
    /// dropped: they describe the samples as they were.
    pub const fn channel_mut(&mut self, c: usize) -> &mut Buffer2<f32> {
        self.samples_mut().channel_mut(c)
    }

    /// The samples, mutably, with the noise facts that described them dropped. Every mutable path
    /// to the samples comes through here, so no change to them keeps a stale noise.
    const fn samples_mut(&mut self) -> &mut PlanarImage {
        self.metadata.derived.after_edit();
        &mut self.samples
    }

    /// The pixels, moved out.
    pub(crate) fn into_pixels(self) -> PlanarImage {
        self.samples
    }

    /// Iterate the channel planes in channel order: one for mono, three for RGB.
    pub(crate) fn planes_mut(&mut self) -> impl Iterator<Item = &mut Buffer2<f32>> {
        self.samples_mut().planes_mut().iter_mut()
    }

    /// The channel planes to edit, in channel order, beside the flags of their pixels: a split
    /// borrow, so an edit that leaves flagged pixels out reads the flags without a copy of them.
    pub(crate) fn planes_mut_with_flags(&mut self) -> PlanesWithFlags<'_> {
        self.metadata.derived.after_edit();
        PlanesWithFlags {
            planes: self.samples.planes_mut().iter_mut(),
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

    /// Per-pixel parallel in-place map that needs all three channels at once. A no-op on a
    /// mono image, which has no cross-channel relationship for `rgb` to act on.
    pub(crate) fn map_rgb(&mut self, rgb: impl Fn(Rgb) -> Rgb + Sync) {
        if !self.is_rgb() {
            return;
        }
        self.rgb_blocks_mut(|_, [r, g, b]| {
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

    /// `block` over the three planes of an RGB image in lockstep, in parallel,
    /// [`SAMPLES_PER_BLOCK`] pixels of every plane at a time, with the block's first pixel.
    pub(crate) fn rgb_blocks_mut(&mut self, block: impl Fn(usize, [&mut [f32]; 3]) + Sync) {
        debug_assert!(self.is_rgb(), "the blocks of an RGB image");
        let [r, g, b] = self.rgb_planes_mut();
        par::chunks_mut(r, SAMPLES_PER_BLOCK)
            .zip(par::chunks_mut(g, SAMPLES_PER_BLOCK))
            .zip(par::chunks_mut(b, SAMPLES_PER_BLOCK))
            .enumerate()
            .for_each(|(idx, ((r, g), b))| block(idx * SAMPLES_PER_BLOCK, [r, g, b]));
    }

    /// The per-pixel combined intensity, read from the channels where it is needed: the channel
    /// itself for mono, `(r+g+b)/3` for RGB.
    pub(crate) fn intensity(&self) -> Intensity<'_> {
        let size = self.desc().size();
        let intensity = if self.is_rgb() {
            Intensity::rgb(size, array::from_fn(|c| self.channel(c).pixels()))
        } else {
            Intensity::mono(size, self.channel(0).pixels())
        };
        intensity.with_flags(self.flags())
    }

    /// The values of `channel`'s pixels holding data an image statistic is taken from — see
    /// [`Intensity::statistic_values`].
    pub(crate) fn statistic_values(&self, channel: usize) -> Vec<f32> {
        Intensity::mono(self.desc().size(), self.channel(channel).pixels())
            .with_flags(self.flags())
            .statistic_values()
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
        self.rgb_blocks_mut(|start, [r, g, b]| {
            let mapped = &mapped.pixels()[start..start + r.len()];
            for (((r, g), b), &m) in r.iter_mut().zip(g.iter_mut()).zip(b.iter_mut()).zip(mapped) {
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

impl From<&LinearImage> for InterleavedImage {
    fn from(linear: &LinearImage) -> Self {
        Self::from(&linear.samples)
    }
}

/// The planes as an image, with no metadata, flags or flat gain.
impl From<PlanarImage> for LinearImage {
    fn from(pixels: PlanarImage) -> Self {
        Self::new(pixels, None, ImageMetadata::default())
    }
}

impl From<LinearImage> for InterleavedImage {
    fn from(linear: LinearImage) -> Self {
        Self::from(&linear)
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

    use crate::image::flags::pixel_flags::PixelFlags;
    use crate::image::flat_gain::FlatGain;
    use crate::image::flat_gain::frame_gain::FrameGain;
    use crate::image::frame::FrameParts;
    use crate::image::linear_image::LinearImage;
    use crate::internals::images::{mono_image, rgb_image};
    use common::USize2;

    /// A 2×1 RGB image holds 6 `f32` samples, 24 bytes; its flag plane adds a byte a pixel, 2; a
    /// gain it holds alone adds its grid, 2 nodes × 3 channels × 4 bytes = 24; a shared one adds
    /// nothing, as its owner counts it.
    #[test]
    fn ram_bytes_counts_the_planes_the_flags_and_an_own_gain() {
        let size = USize2::new(2, 1);
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
        let own = FrameGain::Own(Box::new(gain.clone()));
        assert_eq!(own.own_ram_bytes(), 24);
        image.set_flat_gain(Some(own));
        assert_eq!(image.ram_bytes(), 50);
        let shared = FrameGain::Shared(Arc::new(gain));
        assert_eq!(shared.own_ram_bytes(), 0);
        image.set_flat_gain(Some(shared));
        assert_eq!(image.ram_bytes(), 26);
    }

    /// Taking an image apart and putting it back moves each part: the plane keeps its allocation,
    /// and the metadata, the flags and the gain come back as they went in.
    #[test]
    fn parts_round_trip_without_a_copy() {
        let size = USize2::new(2, 1);
        let mut image = mono_image(size, vec![0.25, 0.5]);
        image.metadata.header.object = Some("M31".to_owned());
        image.set_flags(PixelFlags::from_bytes(Buffer2::new((2, 1), vec![0, 5])));
        let gain = FlatGain::from_nodes(
            size,
            [Buffer2::new((2, 1), vec![1.5, 2.0])]
                .into_iter()
                .collect::<ArrayVec<_, 3>>(),
        );
        image.set_flat_gain(Some(FrameGain::Shared(Arc::new(gain))));
        let plane = image.channel(0).pixels().as_ptr();

        let parts = FrameParts::from(image);
        assert_eq!(parts.samples.channel(0).pixels().as_ptr(), plane);
        let image = LinearImage::from(parts);
        assert_eq!(image.channel(0).pixels().as_ptr(), plane);
        assert_eq!(image.metadata.header.object.as_deref(), Some("M31"));
        assert_eq!(image.flags().map(PixelFlags::bytes), Some(&[0, 5][..]));
        assert!(matches!(image.flat_gain(), Some(FrameGain::Shared(_))));
    }

    #[test]
    #[should_panic(expected = "flags for another geometry")]
    fn parts_refuse_flags_of_another_size() {
        let mut parts = FrameParts::from(mono_image(USize2::new(2, 1), vec![0.25, 0.5]));
        parts.flags = PixelFlags::from_bytes(Buffer2::new((3, 1), vec![0, 5, 0]));
        let _ = LinearImage::from(parts);
    }

    #[test]
    fn map_rgb_maps_every_channel_of_a_pixel_and_skips_mono() {
        // 2x1 RGB: pixels (0.1,0.2,0.3) and (0.4,0.5,0.6).
        let mut image = rgb_image(
            USize2::new(2, 1),
            [vec![0.1, 0.4], vec![0.2, 0.5], vec![0.3, 0.6]],
        );
        image.map_rgb(|px| px.scale(2.0));
        assert_eq!(image.channel(0).pixels(), &[0.2, 0.8]);
        assert_eq!(image.channel(1).pixels(), &[0.4, 1.0]);
        assert_eq!(image.channel(2).pixels(), &[0.6, 1.2]);

        // Mono has no cross-channel relationship for `rgb` to act on, so it is left alone
        // rather than having the closure applied to (l, l, l).
        let mut mono = mono_image(USize2::new(3, 1), vec![0.25, 0.5, 0.75]);
        mono.map_rgb(|px| px.scale(2.0));
        assert_eq!(mono.channel(0).pixels(), &[0.25, 0.5, 0.75]);
    }

    /// The channel mean as f32 rounds it, the sum times the f32 `1/3`: (0.3, 0, 0) gives
    /// `0.3·⅓`, which rounds one ulp above `0.1f32`, to `0x3dcc_ccce`; (0.6, 0.6, 0.6) sums to
    /// `1.8000001` and its third rounds back onto `0.6f32`. Mono is its own intensity.
    #[test]
    fn intensity_plane_is_channel_mean_for_rgb_and_identity_for_mono() {
        let rgb = rgb_image(
            USize2::new(2, 1),
            [vec![0.3, 0.6], vec![0.0, 0.6], vec![0.0, 0.6]],
        );
        assert_eq!(
            rgb.intensity_plane().pixels(),
            &[f32::from_bits(0x3dcc_ccce), 0.6]
        );

        let l = mono_image(USize2::new(2, 1), vec![0.2, 0.7]);
        assert_eq!(l.intensity_plane().pixels(), &[0.2, 0.7]);
    }

    /// Doubling the intensity doubles every channel, `2I / I` being 2 exactly; a mono plane takes
    /// the mapped intensity itself, clamped to `[0, 1]`.
    #[test]
    fn remap_intensity_moves_pixels_with_their_hue() {
        let mut image = rgb_image(USize2::new(1, 1), [vec![0.2], vec![0.1], vec![0.1]]);
        image.remap_intensity(|intensity| Buffer2::new((1, 1), vec![2.0 * intensity.at(0)]));
        assert_eq!(image.channel(0).pixels(), &[0.4]);
        assert_eq!(image.channel(1).pixels(), &[0.2]);
        assert_eq!(image.channel(2).pixels(), &[0.2]);

        let mut mono = mono_image(USize2::new(3, 1), vec![0.2, 0.5, 0.7]);
        mono.remap_intensity(|_| Buffer2::new((3, 1), vec![-0.5, 0.25, 1.5]));
        assert_eq!(mono.channel(0).pixels(), &[0.0, 0.25, 1.0]);
    }
}
