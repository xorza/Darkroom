//! [`LinearPixels`]: the planes of a [`LinearImage`](crate::image::linear_image::LinearImage).

use std::borrow::Borrow;
use std::{array, slice};

use arrayvec::ArrayVec;
use common::Buffer2;
use imaginarium::{ChannelCount, Image};

use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use common::Size2us;

/// Planar floating-point pixels for a monochrome or RGB image.
#[derive(Debug, Clone)]
pub(crate) enum LinearPixels {
    L(Buffer2<f32>),
    Rgb([Buffer2<f32>; 3]),
}

impl LinearPixels {
    pub(crate) fn from_interleaved(dimensions: ImageDimensions, pixels: Vec<f32>) -> Self {
        assert_eq!(
            pixels.len(),
            dimensions.sample_count(),
            "Sample count mismatch: expected {}, got {}",
            dimensions.sample_count(),
            pixels.len()
        );

        if dimensions.is_grayscale() {
            return Buffer2::new(dimensions.size(), pixels).into();
        }

        // Keeps the allocation when its capacity is a whole number of pixels, else copies once.
        let pixels: Vec<[f32; 3]> = bytemuck::try_cast_vec(pixels)
            .unwrap_or_else(|(_, pixels)| bytemuck::pod_collect_to_vec(&pixels));
        Buffer2::new(dimensions.size(), pixels)
            .deinterleave()
            .into()
    }

    pub(crate) fn from_planar_channels(
        dimensions: ImageDimensions,
        channels: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        let expected_pixels_per_channel = dimensions.pixel_count();
        let mut channels = channels.into_iter();

        if dimensions.is_grayscale() {
            let channel = channels.next().expect("Expected 1 channel for grayscale");
            assert_eq!(
                channel.len(),
                expected_pixels_per_channel,
                "Channel 0 pixel count mismatch"
            );
            assert!(channels.next().is_none(), "Too many channels for grayscale");
            return Buffer2::new(dimensions.size(), channel).into();
        }

        let r = channels.next().expect("Expected 3 channels for RGB");
        let g = channels.next().expect("Expected 3 channels for RGB");
        let b = channels.next().expect("Expected 3 channels for RGB");
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
        assert!(channels.next().is_none(), "Too many channels for RGB");
        [
            Buffer2::new(dimensions.size(), r),
            Buffer2::new(dimensions.size(), g),
            Buffer2::new(dimensions.size(), b),
        ]
        .into()
    }

    pub(crate) fn new_zeroed(dimensions: ImageDimensions) -> Self {
        if dimensions.is_grayscale() {
            Buffer2::new_default(dimensions.size()).into()
        } else {
            array::from_fn(|_| Buffer2::new_default(dimensions.size())).into()
        }
    }

    /// The planes of an `L_F32` or `RGB_F32` image, borrowed or taken: a grayscale image taken
    /// keeps its allocation.
    pub(super) fn from_f32_image<I>(image: I) -> Self
    where
        I: Borrow<Image>,
        [Buffer2<f32>; 1]: TryFrom<I, Error = imaginarium::Error>,
        [Buffer2<f32>; 3]: TryFrom<I, Error = imaginarium::Error>,
    {
        match image.borrow().desc().color_format.channel_count {
            ChannelCount::L => {
                let [plane]: [Buffer2<f32>; 1] = image
                    .try_into()
                    .expect("L_F32 image deinterleaves to one f32 plane");
                plane.into()
            }
            ChannelCount::Rgb => {
                let planes: [Buffer2<f32>; 3] = image
                    .try_into()
                    .expect("RGB_F32 image deinterleaves to three f32 planes");
                planes.into()
            }
            ChannelCount::Rgba => panic!("RGBA image must be converted to RGB_F32 first"),
        }
    }

    pub(crate) fn channel(&self, channel: usize) -> &Buffer2<f32> {
        match self {
            Self::L(plane) => {
                assert!(
                    channel == 0,
                    "Grayscale image only has channel 0, got {channel}"
                );
                plane
            }
            Self::Rgb(planes) => {
                assert!(channel < 3, "RGB image has channels 0-2, got {channel}");
                &planes[channel]
            }
        }
    }

    pub(crate) fn channel_mut(&mut self, channel: usize) -> &mut Buffer2<f32> {
        match self {
            Self::L(plane) => {
                assert!(
                    channel == 0,
                    "Grayscale image only has channel 0, got {channel}"
                );
                plane
            }
            Self::Rgb(planes) => {
                assert!(channel < 3, "RGB image has channels 0-2, got {channel}");
                &mut planes[channel]
            }
        }
    }

    pub(crate) fn planes_mut(&mut self) -> slice::IterMut<'_, Buffer2<f32>> {
        match self {
            Self::L(plane) => slice::from_mut(plane).iter_mut(),
            Self::Rgb(planes) => planes.iter_mut(),
        }
    }

    /// The three channel planes' samples, borrowed at once — what a cross-channel per-pixel op
    /// needs to walk R, G and B in step.
    ///
    /// # Panics
    /// On a grayscale image, which has no three planes; callers gate on the `Rgb` variant.
    pub(crate) fn rgb_planes_mut(&mut self) -> [&mut [f32]; 3] {
        match self {
            Self::L(_) => panic!("Expected Rgb variant, got L"),
            Self::Rgb([r, g, b]) => [r.pixels_mut(), g.pixels_mut(), b.pixels_mut()],
        }
    }

    pub(crate) const fn layout(&self) -> ChannelLayout {
        match self {
            Self::L(_) => ChannelLayout::Mono,
            Self::Rgb(_) => ChannelLayout::Rgb,
        }
    }

    pub(crate) const fn channel_count(&self) -> usize {
        self.layout().count()
    }

    pub(crate) fn dimensions(&self) -> ImageDimensions {
        let plane = self.channel(0);
        ImageDimensions::new(plane.size(), self.layout())
    }

    pub(crate) fn into_l(self) -> Buffer2<f32> {
        match self {
            Self::L(plane) => plane,
            Self::Rgb(_) => panic!("Expected L variant, got Rgb"),
        }
    }

    pub(crate) fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3> {
        match self {
            Self::L(plane) => ArrayVec::from_iter([plane]),
            Self::Rgb(planes) => ArrayVec::from(planes),
        }
    }
}

impl From<Buffer2<f32>> for LinearPixels {
    fn from(plane: Buffer2<f32>) -> Self {
        if let Err(error) = ImageDimensions::checked(plane.size(), ChannelLayout::Mono) {
            panic!("{error}");
        }
        Self::L(plane)
    }
}

impl From<[Buffer2<f32>; 3]> for LinearPixels {
    fn from(planes: [Buffer2<f32>; 3]) -> Self {
        let width = planes[0].width();
        let height = planes[0].height();
        if let Err(error) =
            ImageDimensions::checked(Size2us::new(width, height), ChannelLayout::Rgb)
        {
            panic!("{error}");
        }
        for plane in &planes[1..] {
            assert_eq!(plane.width(), width, "all RGB planes must share width");
            assert_eq!(plane.height(), height, "all RGB planes must share height");
        }
        Self::Rgb(planes)
    }
}

/// The planes of an image, one or three of one size.
impl From<ArrayVec<Buffer2<f32>, 3>> for LinearPixels {
    fn from(planes: ArrayVec<Buffer2<f32>, 3>) -> Self {
        let mut planes = planes.into_iter();
        match (planes.next(), planes.next(), planes.next()) {
            (Some(luminance), None, None) => luminance.into(),
            (Some(red), Some(green), Some(blue)) => [red, green, blue].into(),
            _ => panic!("an image has one plane or three"),
        }
    }
}

impl From<&LinearPixels> for Image {
    fn from(pixels: &LinearPixels) -> Self {
        match pixels {
            LinearPixels::L(plane) => Self::from([plane]),
            LinearPixels::Rgb(planes) => Self::from(planes.each_ref()),
        }
    }
}
