use imaginarium::{Buffer2, ChannelCount, Image, PlanarPixels};
use rayon::prelude::*;

use crate::io::image::ImageDimensions;
use crate::math::sum::sum_f32;

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
            return Buffer2::new(dimensions.width(), dimensions.height(), pixels).into();
        }

        let mut r = Buffer2::new_default(dimensions.width(), dimensions.height());
        let mut g = Buffer2::new_default(dimensions.width(), dimensions.height());
        let mut b = Buffer2::new_default(dimensions.width(), dimensions.height());
        deinterleave_rgb(&pixels, r.pixels_mut(), g.pixels_mut(), b.pixels_mut());
        [r, g, b].into()
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
            return Buffer2::new(dimensions.width(), dimensions.height(), channel).into();
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
            Buffer2::new(dimensions.width(), dimensions.height(), r),
            Buffer2::new(dimensions.width(), dimensions.height(), g),
            Buffer2::new(dimensions.width(), dimensions.height(), b),
        ]
        .into()
    }

    pub(crate) fn new_zeroed(dimensions: ImageDimensions) -> Self {
        if dimensions.is_grayscale() {
            Buffer2::new_default(dimensions.width(), dimensions.height()).into()
        } else {
            std::array::from_fn(|_| Buffer2::new_default(dimensions.width(), dimensions.height()))
                .into()
        }
    }

    pub(crate) fn from_f32_image(image: &Image) -> Self {
        match image.desc().color_format.channel_count {
            ChannelCount::L => {
                let planar: PlanarPixels<1, f32> = image
                    .try_into()
                    .expect("L_F32 image deinterleaves to one f32 plane");
                let [plane] = planar.planes;
                plane.into()
            }
            ChannelCount::Rgb => {
                let planar: PlanarPixels<3, f32> = image
                    .try_into()
                    .expect("RGB_F32 image deinterleaves to three f32 planes");
                planar.planes.into()
            }
            ChannelCount::Rgba => panic!("RGBA image must be converted to RGB_F32 first"),
        }
    }

    pub(crate) fn channel(&self, channel: usize) -> &Buffer2<f32> {
        match self {
            LinearPixels::L(plane) => {
                assert!(
                    channel == 0,
                    "Grayscale image only has channel 0, got {channel}"
                );
                plane
            }
            LinearPixels::Rgb(planes) => {
                assert!(channel < 3, "RGB image has channels 0-2, got {channel}");
                &planes[channel]
            }
        }
    }

    pub(crate) fn channel_mut(&mut self, channel: usize) -> &mut Buffer2<f32> {
        match self {
            LinearPixels::L(plane) => {
                assert!(
                    channel == 0,
                    "Grayscale image only has channel 0, got {channel}"
                );
                plane
            }
            LinearPixels::Rgb(planes) => {
                assert!(channel < 3, "RGB image has channels 0-2, got {channel}");
                &mut planes[channel]
            }
        }
    }

    pub(crate) fn channel_count(&self) -> usize {
        match self {
            LinearPixels::L(_) => 1,
            LinearPixels::Rgb(_) => 3,
        }
    }

    pub(crate) fn dimensions(&self) -> ImageDimensions {
        let plane = self.channel(0);
        ImageDimensions::new((plane.width(), plane.height()), self.channel_count())
    }

    pub(crate) fn mean(&self) -> f32 {
        fn parallel_sum(values: &[f32]) -> f32 {
            values.par_chunks(8192).map(sum_f32).sum()
        }

        match self {
            LinearPixels::L(plane) => {
                debug_assert!(!plane.is_empty());
                parallel_sum(plane) / plane.len() as f32
            }
            LinearPixels::Rgb([r, g, b]) => {
                let total = parallel_sum(r) + parallel_sum(g) + parallel_sum(b);
                total / (r.len() + g.len() + b.len()) as f32
            }
        }
    }

    pub(crate) fn into_l(self) -> Buffer2<f32> {
        match self {
            LinearPixels::L(plane) => plane,
            LinearPixels::Rgb(_) => panic!("Expected L variant, got Rgb"),
        }
    }

    pub(crate) fn into_planes(self) -> arrayvec::ArrayVec<Buffer2<f32>, 3> {
        match self {
            LinearPixels::L(plane) => arrayvec::ArrayVec::from_iter([plane]),
            LinearPixels::Rgb(planes) => arrayvec::ArrayVec::from_iter(planes),
        }
    }
}

impl From<Buffer2<f32>> for LinearPixels {
    fn from(plane: Buffer2<f32>) -> Self {
        ImageDimensions::new((plane.width(), plane.height()), 1);
        LinearPixels::L(plane)
    }
}

impl From<[Buffer2<f32>; 3]> for LinearPixels {
    fn from(planes: [Buffer2<f32>; 3]) -> Self {
        let width = planes[0].width();
        let height = planes[0].height();
        ImageDimensions::new((width, height), 3);
        for plane in &planes[1..] {
            assert_eq!(plane.width(), width, "all RGB planes must share width");
            assert_eq!(plane.height(), height, "all RGB planes must share height");
        }
        LinearPixels::Rgb(planes)
    }
}

impl From<&LinearPixels> for Image {
    fn from(pixels: &LinearPixels) -> Self {
        match pixels {
            LinearPixels::L(plane) => Image::from([plane]),
            LinearPixels::Rgb(planes) => Image::from(planes.each_ref()),
        }
    }
}

fn deinterleave_rgb(interleaved: &[f32], r: &mut [f32], g: &mut [f32], b: &mut [f32]) {
    debug_assert_eq!(interleaved.len(), r.len() * 3);
    debug_assert_eq!(r.len(), g.len());
    debug_assert_eq!(g.len(), b.len());

    r.par_iter_mut()
        .zip(g.par_iter_mut())
        .zip(b.par_iter_mut())
        .enumerate()
        .for_each(|(index, ((r, g), b))| {
            let source = index * 3;
            *r = interleaved[source];
            *g = interleaved[source + 1];
            *b = interleaved[source + 2];
        });
}

#[cfg(test)]
pub(crate) mod test_support {
    use rayon::prelude::*;

    use crate::io::image::linear_pixels::LinearPixels;

    pub(crate) fn into_interleaved_pixels(pixels: LinearPixels) -> Vec<f32> {
        match pixels {
            LinearPixels::L(plane) => plane.into_vec(),
            LinearPixels::Rgb([r, g, b]) => {
                let mut interleaved = vec![0.0; r.len() * 3];
                interleaved
                    .par_chunks_mut(3)
                    .enumerate()
                    .for_each(|(index, rgb)| {
                        rgb[0] = r[index];
                        rgb[1] = g[index];
                        rgb[2] = b[index];
                    });
                interleaved
            }
        }
    }
}
