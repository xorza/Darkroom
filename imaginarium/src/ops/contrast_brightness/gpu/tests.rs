use crate::common::color_format::ALL_FORMATS;
use crate::common::image_diff::assert_matches_cpu;
use crate::common::internals::create_test_image;
use crate::common::internals::gpu::test_gpu;
use crate::gpu::gpu_image::GpuImage;
use crate::ops::contrast_brightness::ContrastBrightness;
use crate::ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline;

/// Every format under the identity, a contrast and brightness that saturate part of the
/// range, and a pure brightness shift, against the CPU reference — one pipeline for all.
#[test]
fn gpu_matches_cpu() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    let pipeline = GpuContrastBrightnessPipeline::new(&gpu);
    for format in ALL_FORMATS {
        let image = create_test_image(format, 13, 5, 7);
        let input = GpuImage::from_image(&gpu, &image).unwrap();
        for params in [
            ContrastBrightness::new(1.0, 0.0),
            ContrastBrightness::new(1.7, -0.15),
            ContrastBrightness::new(0.6, 0.2),
        ] {
            let mut output = GpuImage::new_empty(&gpu, input.desc).unwrap();
            params.apply_gpu(&gpu, &pipeline, &input, &mut output);

            let mut expected = image.clone();
            params.apply_cpu(&mut expected);
            assert_matches_cpu(
                &output.to_image(&gpu).unwrap(),
                &expected,
                &format!("{format} {params:?}"),
            );
        }
    }
}
