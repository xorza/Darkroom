use glam::Vec2;

use crate::common::color_format::ALL_FORMATS;
use crate::common::image_diff::assert_matches_cpu;
use crate::common::internals::create_test_image;
use crate::common::internals::gpu::test_gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
use crate::ops::transform::pipeline::GpuTransformPipeline;
use crate::ops::transform::{FilterMode, Transform};

/// Every format against the CPU reference. Nearest sampling picks a pixel on a rounding
/// boundary, which a few ULP of position can move, so it runs only where every sample
/// position is exact — the identity and a doubling; bilinear is continuous in the
/// position and runs under a rotation and a shrink as well.
#[test]
fn gpu_matches_cpu() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    let pipeline = GpuTransformPipeline::new(&gpu);
    let (width, height) = (13, 9);
    let center = Vec2::new(6.5, 4.5);
    let cases = [
        (Transform::new(), [width, height], true),
        (
            Transform::new().scale(Vec2::splat(2.0)),
            [2 * width, 2 * height],
            true,
        ),
        (
            Transform::new().rotate_around(0.3, center),
            [width, height],
            false,
        ),
        (Transform::new().scale(Vec2::new(0.5, 0.75)), [7, 7], false),
    ];
    for format in ALL_FORMATS {
        let image = create_test_image(format, width, height, 3);
        let input = GpuImage::from_image(&gpu, &image).unwrap();
        for (transform, [out_w, out_h], exact) in cases {
            let filters: &[FilterMode] = if exact {
                &[FilterMode::Nearest, FilterMode::Bilinear]
            } else {
                &[FilterMode::Bilinear]
            };
            for &filter in filters {
                let transform = transform.filter(filter);
                let desc = ImageDesc::new(out_w, out_h, format);
                let mut output = GpuImage::new_empty(&gpu, desc).unwrap();
                transform.apply_gpu(&gpu, &pipeline, &input, &mut output);

                let mut expected = Image::new_black(desc).unwrap();
                transform.apply_cpu(&image, &mut expected);
                assert_matches_cpu(
                    &output.to_image(&gpu).unwrap(),
                    &expected,
                    &format!("{format} {filter:?} {transform:?}"),
                );
            }
        }
    }
}
