use crate::color::color_format::ColorFormat;
use crate::gpu::gpu_image::GpuImage;
use crate::internals::create_test_image;
use crate::internals::gpu::test_gpu;
use crate::internals::image_diff::assert_matches_cpu;
use crate::ops::contrast_brightness::ContrastBrightness;
use common::Size2us;

/// Every format under the identity, a contrast and brightness that saturate part of the
/// range, and a pure brightness shift, against the CPU reference, each in place on a fresh
/// upload — one kernel for all.
#[test]
fn gpu_matches_cpu() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    for format in ColorFormat::ALL {
        let image = create_test_image(format, Size2us::new(13, 5), 7);
        for params in [
            ContrastBrightness::new(1.0, 0.0),
            ContrastBrightness::new(1.7, -0.15),
            ContrastBrightness::new(0.6, 0.2),
        ] {
            let mut output = GpuImage::from_image(gpu, &image).unwrap();
            params.apply_gpu(gpu, &mut output);

            let mut expected = image.clone();
            params.apply_cpu(&mut expected);
            assert_matches_cpu(
                &output.to_image(gpu).unwrap(),
                &expected,
                &format!("{format} {params:?}"),
            );
        }
    }
}
