use common::USize2;
use strum::IntoEnumIterator;

use crate::color::color_format::ColorFormat;
use crate::gpu::gpu_image::GpuImage;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::internals::gpu::test_gpu;
use crate::internals::image_diff::assert_matches_cpu;
use crate::ops::blend::{Blend, BlendMode};
use crate::ops::clamp::Clamp;

/// Every format, mode, clamp and a spread of alphas against the CPU reference. One kernel
/// serves every dispatch, so a reused kernel that kept anything of an earlier run
/// shows up as a mismatch on a later one.
#[test]
fn gpu_matches_cpu() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    for format in ColorFormat::ALL {
        let src_cpu = create_test_image(format, USize2::new(13, 5), 0);
        let dst_cpu = create_test_image(format, USize2::new(13, 5), 100);
        let src = GpuImage::from_image(gpu, &src_cpu).unwrap();
        let dst = GpuImage::from_image(gpu, &dst_cpu).unwrap();
        for (mode, clamp) in
            BlendMode::iter().flat_map(|mode| Clamp::iter().map(move |clamp| (mode, clamp)))
        {
            for alpha in [0.0, 0.35, 1.0] {
                let params = Blend::new(mode, alpha).clamp(clamp);
                let mut output = GpuImage::new_empty(gpu, dst.desc).unwrap();
                params.apply_gpu(gpu, &src, &dst, &mut output);

                let mut expected = InterleavedImage::new_zeroed(dst_cpu.desc());
                params.apply_cpu(&src_cpu, &dst_cpu, &mut expected);
                assert_matches_cpu(
                    &output.to_image(gpu).unwrap(),
                    &expected,
                    &format!("{format} {mode:?} {clamp:?} alpha {alpha}"),
                );
            }
        }
    }
}
