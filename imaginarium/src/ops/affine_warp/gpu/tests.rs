use common::USize2;
use glam::Vec2;

use crate::color::color_format::ColorFormat;
use crate::gpu::gpu_image::GpuImage;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::internals::gpu::test_gpu;
use crate::internals::image_diff::assert_matches_cpu;
use crate::ops::affine_warp::{AffineWarp, FilterMode};

/// Every format against the CPU reference. Nearest sampling picks a pixel on a rounding
/// boundary, which a few ULP of position can move, so it runs only where every sample
/// position is exact — the identity and a doubling; bilinear is continuous in the
/// position and runs under a rotation and a shrink as well.
#[test]
fn gpu_matches_cpu() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    let (width, height) = (13, 9);
    let center = Vec2::new(6.5, 4.5);
    let cases = [
        (AffineWarp::new(), [width, height], true),
        (
            AffineWarp::new().scale(Vec2::splat(2.0)),
            [2 * width, 2 * height],
            true,
        ),
        (
            AffineWarp::new().rotate_around(0.3, center),
            [width, height],
            false,
        ),
        (AffineWarp::new().scale(Vec2::new(0.5, 0.75)), [7, 7], false),
    ];
    for format in ColorFormat::ALL {
        let image = create_test_image(format, USize2::new(width, height), 3);
        let input = GpuImage::from_image(gpu, &image).unwrap();
        for (warp, [out_w, out_h], exact) in cases {
            let filters: &[FilterMode] = if exact {
                &[FilterMode::Nearest, FilterMode::Bilinear]
            } else {
                &[FilterMode::Bilinear]
            };
            for &filter in filters {
                let warp = warp.filter(filter);
                let desc = InterleavedDesc::new(USize2::new(out_w, out_h), format);
                let mut output = GpuImage::new_empty(gpu, desc).unwrap();
                warp.apply_gpu(gpu, &input, &mut output);

                let mut expected = InterleavedImage::new_zeroed(desc);
                warp.apply_cpu(&image, &mut expected);
                assert_matches_cpu(
                    &output.to_image(gpu).unwrap(),
                    &expected,
                    &format!("{format} {filter:?} {warp:?}"),
                );
            }
        }
    }
}
