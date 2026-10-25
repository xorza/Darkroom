use common::Size2us;
use strum::IntoEnumIterator;

use crate::color::color_format::ColorFormat;
use crate::gpu::gpu_image::GpuImage;
use crate::image::InterleavedImage;
use crate::internals::create_test_image;
use crate::internals::gpu::test_gpu;
use crate::internals::image_diff::assert_matches_cpu;
use crate::ops::blend::pipeline::GpuBlendPipeline;
use crate::ops::blend::{Blend, BlendMode};

/// Every format, mode and a spread of alphas against the CPU reference. One pipeline
/// serves every dispatch, so a reused pipeline that kept anything of an earlier run
/// shows up as a mismatch on a later one.
#[test]
fn gpu_matches_cpu() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    let pipeline = GpuBlendPipeline::new(&gpu);
    for format in ColorFormat::ALL {
        let src_cpu = create_test_image(format, Size2us::new(13, 5), 0);
        let dst_cpu = create_test_image(format, Size2us::new(13, 5), 100);
        let src = GpuImage::from_image(&gpu, &src_cpu).unwrap();
        let dst = GpuImage::from_image(&gpu, &dst_cpu).unwrap();
        for mode in BlendMode::iter() {
            for alpha in [0.0, 0.35, 1.0] {
                let params = Blend::new(mode, alpha);
                let mut output = GpuImage::new_empty(&gpu, dst.desc).unwrap();
                params.apply_gpu(&gpu, &pipeline, &src, &dst, &mut output);

                let mut expected = InterleavedImage::new_black(dst_cpu.desc()).unwrap();
                params.apply_cpu(&src_cpu, &dst_cpu, &mut expected);
                assert_matches_cpu(
                    &output.to_image(&gpu).unwrap(),
                    &expected,
                    &format!("{format} {mode:?} alpha {alpha}"),
                );
            }
        }
    }
}
