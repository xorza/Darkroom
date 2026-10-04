//! [`PipelineFrame`]: the frame carrier the registered-stacking pipeline moves between stages.

use std::borrow::Cow;

use crate::frame_store::stored_image::StoredImage;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};

/// A calibrated frame waiting to be registered, held wherever the memory tier put it.
///
/// The two variants are the whole difference between the all-RAM and the memory-bounded runs:
/// the pipeline body downstream is identical, because the warp reads either through one
/// [`SourceImage`] — a spilled frame's planes in place from its map, never copied back.
#[derive(Debug)]
pub(crate) enum PipelineFrame {
    Resident(LinearImage),
    Spilled(StoredImage),
}

impl PipelineFrame {
    /// The frame as the warp reads it.
    pub(crate) fn source(&self) -> SourceImage<'_> {
        match self {
            Self::Resident(image) => SourceImage::of(image),
            Self::Spilled(stored) => {
                let width = stored.dimensions.width();
                SourceImage {
                    dimensions: stored.dimensions,
                    planes: stored
                        .planes()
                        .map(|pixels| SourcePlane { pixels, width })
                        .collect(),
                    flags: stored.flags().map(Cow::Owned),
                }
            }
        }
    }

    pub(crate) const fn metadata(&self) -> &ImageMetadata {
        match self {
            Self::Resident(image) => &image.metadata,
            Self::Spilled(stored) => &stored.metadata,
        }
    }

    pub(crate) fn dimensions(&self) -> ImageDimensions {
        match self {
            Self::Resident(image) => image.dimensions(),
            Self::Spilled(stored) => stored.dimensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::TempDir;

    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_image::StoredImage;
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::linear::LinearImage;
    use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
    use crate::pipeline::pipeline_frame::PipelineFrame;

    /// A parked frame reaches the warp as its map: the source's planes are the map's own memory,
    /// not a copy, and its flags come with it.
    #[test]
    fn a_parked_frame_is_warped_from_its_map_in_place() {
        let directory = TempDir::new("pipeline_frame_source");
        let scratch = RunScratch::create(directory.path()).unwrap();
        let dimensions = ImageDimensions::new((3, 2), 3);
        let mut image = LinearImage::from_planar_channels(
            dimensions,
            [
                vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
                vec![6.0; 6],
                vec![-1.0; 6],
            ],
        );
        image.flags =
            PixelFlags::of_non_finite(dimensions.size(), &[&[0.0, 0.0, f32::NAN, 0.0, 0.0, 0.0]]);
        let stored = StoredImage::spill(&scratch, &image).unwrap();
        let mapped: Vec<*const f32> = stored.planes().map(<[f32]>::as_ptr).collect();
        let frame = PipelineFrame::Spilled(stored);
        let source = frame.source();
        assert_eq!(source.dimensions, dimensions);
        for (plane, (&pointer, expected)) in source.planes.iter().zip(
            mapped
                .iter()
                .zip((0..3).map(|channel| image.channel(channel))),
        ) {
            assert_eq!(plane.pixels.as_ptr(), pointer, "a plane was copied");
            assert_eq!(plane.pixels, expected.pixels());
            assert_eq!(plane.width, 3);
        }
        assert_eq!(source.flags.unwrap().count(QualityFlags::NO_DATA), 1);
    }
}
