//! [`PlanarImage`]: `f32` planes, one or three, and the lossless conversions between them and an
//! [`InterleavedImage`].

use std::array;

use common::Buffer2;

use crate::color::color_format::{ChannelCount, ColorFormat};
use crate::error::{DescError, PlanesError};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::planar::channel_layout::ChannelLayout;
use crate::planar::planar_desc::PlanarDesc;

/// `f32` planes of one size: one for a mono image, three for RGB.
#[derive(Debug, Clone)]
pub struct PlanarImage {
    planes: Planes,
}

#[derive(Debug, Clone)]
enum Planes {
    Mono(Buffer2<f32>),
    Rgb([Buffer2<f32>; 3]),
}

impl PlanarImage {
    /// The planes of `samples`, interleaved in `desc`'s channel order (RGBRGB…). An RGB image
    /// keeps the allocation when its capacity is a whole number of pixels, and copies once when it
    /// is not; a mono image keeps it always.
    ///
    /// # Panics
    /// Unless `samples` holds `desc`'s sample count.
    pub fn from_interleaved(desc: PlanarDesc, samples: Vec<f32>) -> Self {
        assert_eq!(
            samples.len(),
            desc.sample_count(),
            "{desc} needs {} samples",
            desc.sample_count()
        );
        if desc.is_mono() {
            return Self::new_mono(Buffer2::new(desc.size(), samples));
        }
        let pixels: Vec<[f32; 3]> = bytemuck::try_cast_vec(samples)
            .unwrap_or_else(|(_, samples)| bytemuck::pod_collect_to_vec(&samples));
        Self::new_rgb(Buffer2::new(desc.size(), pixels).deinterleave())
    }

    /// A mono image of `plane`, its allocation kept.
    ///
    /// # Panics
    /// When `plane` holds no pixel.
    pub fn new_mono(plane: Buffer2<f32>) -> Self {
        Self::checked_mono(plane).unwrap_or_else(|error| panic!("{error}"))
    }

    /// [`Self::new_mono`], or why `plane` is no image.
    pub fn checked_mono(plane: Buffer2<f32>) -> Result<Self, DescError> {
        PlanarDesc::checked(plane.size(), ChannelLayout::Mono)?;
        Ok(Self {
            planes: Planes::Mono(plane),
        })
    }

    /// An RGB image of `planes`, each kept as its allocation.
    ///
    /// # Panics
    /// Unless the planes share a size that holds a pixel.
    pub fn new_rgb(planes: [Buffer2<f32>; 3]) -> Self {
        Self::checked_rgb(planes).unwrap_or_else(|error| panic!("{error}"))
    }

    /// [`Self::new_rgb`], or why `planes` are no image.
    pub fn checked_rgb(planes: [Buffer2<f32>; 3]) -> Result<Self, DescError> {
        let expected = planes[0].size();
        PlanarDesc::checked(expected, ChannelLayout::Rgb)?;
        if let Some(plane) = planes[1..].iter().find(|plane| plane.size() != expected) {
            return Err(DescError::PlaneSize {
                expected,
                got: plane.size(),
            });
        }
        Ok(Self {
            planes: Planes::Rgb(planes),
        })
    }

    /// The planes of `image` with its alpha channel dropped, each sample on the unit scale: the one
    /// conversion to planes that loses data, which a caller names. An image with no alpha gives
    /// what [`PlanarImage::try_from`] gives.
    pub fn without_alpha(image: &InterleavedImage) -> Self {
        image.unit_planes()
    }

    /// The planes `planes` holds, in channel order, each kept as its allocation.
    ///
    /// # Panics
    /// Unless `planes` holds one plane per channel of `desc`, each of `desc`'s pixel count.
    pub fn from_planar_channels(
        desc: PlanarDesc,
        planes: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        let mut planes = planes.into_iter().map(|plane| {
            assert_eq!(
                plane.len(),
                desc.pixel_count(),
                "a plane of {desc} holds {} samples",
                desc.pixel_count()
            );
            Buffer2::new(desc.size(), plane)
        });
        let mut next = || {
            planes
                .next()
                .unwrap_or_else(|| panic!("{desc} needs more planes"))
        };
        let image = match desc.layout() {
            ChannelLayout::Mono => Self::new_mono(next()),
            ChannelLayout::Rgb => Self::new_rgb([next(), next(), next()]),
        };
        assert!(planes.next().is_none(), "{desc} holds fewer planes");
        image
    }

    /// Black planes of `desc`.
    pub fn new_zeroed(desc: PlanarDesc) -> Self {
        match desc.layout() {
            ChannelLayout::Mono => Self::new_mono(Buffer2::new_default(desc.size())),
            ChannelLayout::Rgb => {
                Self::new_rgb(array::from_fn(|_| Buffer2::new_default(desc.size())))
            }
        }
    }

    pub fn desc(&self) -> PlanarDesc {
        PlanarDesc::new(self.channel(0).size(), self.layout())
    }

    pub const fn layout(&self) -> ChannelLayout {
        match self.planes {
            Planes::Mono(_) => ChannelLayout::Mono,
            Planes::Rgb(_) => ChannelLayout::Rgb,
        }
    }

    /// How many planes the image holds.
    pub const fn channel_count(&self) -> usize {
        self.layout().count()
    }

    pub const fn is_mono(&self) -> bool {
        matches!(self.planes, Planes::Mono(_))
    }

    pub const fn is_rgb(&self) -> bool {
        matches!(self.planes, Planes::Rgb(_))
    }

    /// The plane of channel `idx`.
    ///
    /// # Panics
    /// Unless the image holds channel `idx`.
    pub const fn channel(&self, idx: usize) -> &Buffer2<f32> {
        &self.planes()[idx]
    }

    /// [`Self::channel`], mutable.
    pub const fn channel_mut(&mut self, idx: usize) -> &mut Buffer2<f32> {
        &mut self.planes_mut()[idx]
    }

    /// Every plane, in channel order.
    pub const fn planes(&self) -> &[Buffer2<f32>] {
        match &self.planes {
            Planes::Mono(plane) => std::slice::from_ref(plane),
            Planes::Rgb(planes) => planes,
        }
    }

    /// [`Self::planes`], mutable.
    pub const fn planes_mut(&mut self) -> &mut [Buffer2<f32>] {
        match &mut self.planes {
            Planes::Mono(plane) => std::slice::from_mut(plane),
            Planes::Rgb(planes) => planes,
        }
    }

    /// The three planes' samples, borrowed at once: what a cross-channel op walks R, G and B in
    /// step through.
    ///
    /// # Panics
    /// On a mono image.
    pub fn rgb_planes_mut(&mut self) -> [&mut [f32]; 3] {
        match &mut self.planes {
            Planes::Mono(_) => panic!("a mono image has no three planes"),
            Planes::Rgb([r, g, b]) => [r.pixels_mut(), g.pixels_mut(), b.pixels_mut()],
        }
    }

    /// The one plane of a mono image.
    ///
    /// # Panics
    /// On an RGB image.
    pub fn into_mono(self) -> Buffer2<f32> {
        match self.planes {
            Planes::Mono(plane) => plane,
            Planes::Rgb(_) => panic!("an RGB image has three planes"),
        }
    }

    /// The three planes of an RGB image.
    ///
    /// # Panics
    /// On a mono image.
    pub fn into_rgb(self) -> [Buffer2<f32>; 3] {
        match self.planes {
            Planes::Mono(_) => panic!("a mono image has one plane"),
            Planes::Rgb(planes) => planes,
        }
    }
}

/// An image of `format` refused as planes when it has alpha: planes hold none.
const fn refuse_alpha(format: ColorFormat) -> Result<(), PlanesError> {
    match format.channel_count {
        ChannelCount::Mono | ChannelCount::Rgb => Ok(()),
        ChannelCount::Rgba => Err(PlanesError::Alpha {
            color_format: format,
        }),
    }
}

/// The planes interleaved, taking them: a mono image hands its plane over as its `MONO_F32`
/// samples.
impl From<PlanarImage> for InterleavedImage {
    fn from(image: PlanarImage) -> Self {
        match image.planes {
            Planes::Mono(plane) => Self::from_samples(
                InterleavedDesc::new(plane.size(), ColorFormat::MONO_F32),
                Vec::from(plane),
            ),
            Planes::Rgb(planes) => Self::from(planes.each_ref()),
        }
    }
}

/// The planes interleaved, as `MONO_F32` or `RGB_F32`.
impl From<&PlanarImage> for InterleavedImage {
    fn from(image: &PlanarImage) -> Self {
        match &image.planes {
            Planes::Mono(plane) => Self::from([plane]),
            Planes::Rgb(planes) => Self::from(planes.each_ref()),
        }
    }
}

/// The planes of a mono or RGB image in any sample type, each sample on the unit scale, which
/// every `u8` and `u16` value survives exactly. An image with alpha is refused:
/// [`PlanarImage::without_alpha`] names the drop.
impl TryFrom<&InterleavedImage> for PlanarImage {
    type Error = PlanesError;

    fn try_from(image: &InterleavedImage) -> Result<Self, PlanesError> {
        refuse_alpha(image.desc().color_format())?;
        Ok(image.unit_planes())
    }
}

/// The borrowing conversion, taking the image: a `MONO_F32` image keeps its allocation as its
/// plane.
impl TryFrom<InterleavedImage> for PlanarImage {
    type Error = PlanesError;

    fn try_from(image: InterleavedImage) -> Result<Self, PlanesError> {
        refuse_alpha(image.desc().color_format())?;
        if image.desc().color_format() != ColorFormat::MONO_F32 {
            return Ok(image.unit_planes());
        }
        let [plane] = image.into_planes::<f32, 1>().expect("a MONO_F32 image");
        Ok(Self::new_mono(plane))
    }
}

#[cfg(test)]
mod tests;
