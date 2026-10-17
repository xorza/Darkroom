use common::TempDir;

use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_image::StoredImage;
use crate::image::channel_layout::ChannelLayout;
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
    let dimensions = ImageDimensions::new((3, 2), ChannelLayout::Rgb);
    let mut image = LinearImage::from_planar_channels(
        dimensions,
        [
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            vec![6.0; 6],
            vec![-1.0; 6],
        ],
    );
    image.set_flags(PixelFlags::of_non_finite(
        dimensions.size(),
        &[&[0.0, 0.0, f32::NAN, 0.0, 0.0, 0.0]],
    ));
    let stored = StoredImage::spill(&scratch, &image).unwrap();
    let mapped: Vec<*const f32> = stored.planes().map(<[f32]>::as_ptr).collect();
    let resident = PipelineFrame::Spilled(stored).into_resident();
    assert_eq!(resident.dimensions(), dimensions);
    for (channel, &pointer) in mapped.iter().enumerate() {
        let plane = resident.channel(channel).pixels();
        assert_ne!(plane.as_ptr(), pointer, "channel {channel} reads the map");
        assert_eq!(plane, image.channel(channel).pixels());
    }
    assert_eq!(resident.flags().unwrap().count(QualityFlags::NO_DATA), 1);
}
