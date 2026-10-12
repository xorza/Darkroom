//! [`PipelineFrame`]: the frame carrier the registered-stacking pipeline moves between stages.

use crate::frame_store::stored_image::StoredImage;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;

/// A calibrated frame waiting to be registered, held wherever the memory tier put it.
///
/// The two variants are the whole difference between the all-RAM and the memory-bounded runs:
/// the pipeline body downstream is identical, because the warp reads either from RAM — a spilled
/// frame read back whole first ([`Self::into_resident`]).
#[derive(Debug)]
pub(crate) enum PipelineFrame {
    Resident(LinearImage),
    Spilled(StoredImage),
}

impl PipelineFrame {
    /// The frame in RAM, as the warp reads it. A parked frame is read back whole, one pass front
    /// to back over each of its planes: the warp then reads in whatever order its transform asks —
    /// a column per output row for a quarter turn — from memory the plan already charges for its
    /// source, not by faulting pages of a map the plan does not count.
    pub(crate) fn into_resident(self) -> LinearImage {
        match self {
            Self::Resident(image) => image,
            Self::Spilled(stored) => stored.load(),
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
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::pipeline::pipeline_frame::PipelineFrame;

    /// A parked frame reaches the warp read back whole: its planes in RAM, not the map's pages,
    /// holding what was parked, and its flags with them.
    #[test]
    fn a_parked_frame_is_read_back_whole_before_its_warp() {
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
        let resident = PipelineFrame::Spilled(stored).into_resident();
        assert_eq!(resident.dimensions(), dimensions);
        for (channel, &pointer) in mapped.iter().enumerate() {
            let plane = resident.channel(channel).pixels();
            assert_ne!(plane.as_ptr(), pointer, "channel {channel} reads the map");
            assert_eq!(plane, image.channel(channel).pixels());
        }
        assert_eq!(resident.flags.unwrap().count(QualityFlags::NO_DATA), 1);
    }
}
