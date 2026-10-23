use std::array;

use imaginarium::Buffer2;

use crate::image_ops::ml::backend::*;
use crate::math::size2us::Size2us;

/// A plane whose every sample names its pixel and channel: `c/4 + (x + y·w)/(4·w·h)`, inside
/// `[c/4, c/4 + 1/4)` so nothing reaches the clamp, and distinct at f32 resolution — a wrong tile
/// origin, a wrong row stride or a swapped channel reads a different value.
fn ramp(width: usize, height: usize, channel: usize) -> Buffer2<f32> {
    let count = (width * height) as f32;
    let pixels = (0..width * height)
        .map(|i| channel as f32 / 4.0 + i as f32 / (4.0 * count))
        .collect();
    Buffer2::new(width, height, pixels)
}

#[test]
fn a_tile_reads_from_its_own_origin_with_each_channel_in_its_own_model_slot() {
    // 640² so the tile origin is not forced to (0,0) and an off-by-one in the row stride shows.
    let (side, tile) = (640usize, Vec2us::new(128, 96));
    let planar = LinearImage::from(array::from_fn::<_, 3, _>(|c| ramp(side, side, c)));
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    fill_tile_input(&planar, tile, &mut input);

    for hh in 0..WINDOW {
        for ww in 0..WINDOW {
            let dst = (hh * WINDOW + ww) * 3;
            let src = (tile.y + hh) * side + tile.x + ww;
            for c in 0..3 {
                assert_eq!(
                    input[dst + c],
                    planar.channel(c).pixels()[src],
                    "channel {c} at model ({ww}, {hh})"
                );
            }
        }
    }
}

#[test]
fn a_mono_master_replicates_into_all_three_model_channels() {
    // The net is RGB-only, so a grayscale master must arrive as R=G=B rather than leaving two
    // channels at zero — and the clamp is what keeps a sub-background or star-core sample in
    // the [0,1] domain the model was trained on.
    let side = WINDOW;
    let plane = Buffer2::from_fn(side, side, |x, y| match (x, y) {
        (0, 0) => -0.25,
        (1, 0) => 1.5,
        _ => 0.5,
    });
    let planar = LinearImage::from(plane);
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    fill_tile_input(&planar, Vec2us::new(0, 0), &mut input);

    for (pixel, expected) in [(0usize, 0.0f32), (1, 1.0), (2, 0.5), (side * side - 1, 0.5)] {
        assert_eq!(
            &input[pixel * 3..pixel * 3 + 3],
            &[expected; 3],
            "pixel {pixel}"
        );
    }
}

/// Tile origins step by the stride until a window reaches the edge, and that last one sits flush
/// against it: 640 at 448 is 0 then 640 − 512 = 128; 1500 at 448 is 0, 448, 896, then 988; 960 at
/// 448 lands its second window exactly on the edge. A frame one window wide is one tile.
#[test]
fn tile_starts_step_by_the_stride_and_end_flush() {
    for (dim, stride, starts) in [
        (512usize, 256usize, &[0usize][..]),
        (640, 448, &[0, 128]),
        (1500, 448, &[0, 448, 896, 988]),
        (960, 448, &[0, 448]),
        (700, 256, &[0, 188]),
    ] {
        assert_eq!(tile_starts(dim, stride), starts, "{dim} at {stride}");
    }
}

/// The feather weight is the distance from the nearer tile edge over the 64-px ramp, floored at
/// [`FEATHER_MIN`]: the first two pixels take the floor (1/64 is under 0.02), 2 px in takes 2/64,
/// 32 px ½, and from 64 px in the full weight — symmetric about the tile centre. Every ratio is
/// dyadic, so exact.
#[test]
fn feather_ramps_from_either_edge() {
    for (i, weight) in [
        (0usize, FEATHER_MIN),
        (1, FEATHER_MIN),
        (2, 2.0 / 64.0),
        (32, 0.5),
        (64, 1.0),
        (255, 1.0),
    ] {
        assert_eq!(feather(i), weight, "{i}");
        assert_eq!(
            feather(WINDOW - 1 - i),
            weight,
            "{} mirrors {i}",
            WINDOW - 1 - i
        );
    }
}

/// A model that returns its input, run over every tile of a frame two tiles a side, blends back to
/// the input. Each pixel is `Σ v·wₖ / Σ wₖ` over at most four tiles: four products, three sums
/// above and three below the line, and the division round by half an ulp each — 5.5ε of `v`.
/// Gray averages its three equal channels first, two sums and a product more: 8ε bounds both. The
/// blend lands in the frame's planes, which keep the frame's metadata.
#[test]
fn an_identity_model_reproduces_the_input() {
    let size = Size2us::new(700, 600);
    let rgb = LinearImage::from(array::from_fn::<_, 3, _>(|c| {
        ramp(size.width, size.height, c)
    }));
    let gray = LinearImage::from(ramp(size.width, size.height, 1));
    for image in [rgb, gray] {
        let mut output = image.clone();
        output.metadata.header.exposure_time = Some(30.0);
        run_tiles(
            Planes::InPlace(&mut output),
            OnnxModel::DEFAULT_STRIDE,
            |input, tile| {
                tile.copy_from_slice(input);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(output.channels(), image.channels());
        assert_eq!(output.metadata.header.exposure_time, Some(30.0));
        for c in 0..image.channels() {
            for (i, (&got, &expected)) in output
                .channel(c)
                .pixels()
                .iter()
                .zip(image.channel(c).pixels())
                .enumerate()
            {
                assert!(
                    (got - expected).abs() <= 8.0 * f32::EPSILON * expected,
                    "channel {c}, pixel {i}: {got} vs {expected}"
                );
            }
        }
    }
}

/// The stride covers the frame at full weight or is refused before any tile runs: 0 never
/// advances, and past 512 − 2·64 = 384 a band between tiles lies only on their feather ramps. 1
/// and 384 are the edges that run. At 384 the full-weight cores, positions 64 to 447 of each
/// window, meet: the next core starts at 384 + 64 = 448.
#[test]
fn the_stride_must_cover_the_frame() {
    assert_eq!(MAX_STRIDE, 384);
    let core: Vec<usize> = (0..WINDOW).filter(|&i| feather(i) == 1.0).collect();
    assert_eq!(
        (core[0], core[core.len() - 1], core.len()),
        (64, 447, MAX_STRIDE)
    );
    for stride in [1, 256, MAX_STRIDE] {
        assert_eq!(validate_stride(stride), Ok(()), "{stride}");
    }
    for stride in [0, MAX_STRIDE + 1, WINDOW] {
        let mut image = LinearImage::from(Buffer2::new_filled(WINDOW, WINDOW, 0.5f32));
        let error = run_tiles(Planes::InPlace(&mut image), stride, |_, _| {
            panic!("a stride refused runs no tile")
        })
        .unwrap_err();
        assert!(
            matches!(&error, MlError::Config(invalid) if invalid.field == "ML tile stride"),
            "{stride}: {error:?}"
        );
    }
}

/// The whole-frame blend the band replaced: every tile's feathered sums over the frame, normalized
/// once every tile is in, a grayscale frame the mean of the three channels.
fn whole_frame_blend(
    image: &LinearImage,
    stride: usize,
    mut infer: impl FnMut(&[f32], &mut [f32]),
) -> Vec<Vec<f32>> {
    let (width, height) = (image.width(), image.height());
    let mut sums = [
        vec![0.0f32; width * height],
        vec![0.0; width * height],
        vec![0.0; width * height],
    ];
    let mut weight = vec![0.0f32; width * height];
    let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
    let mut output = vec![0.0f32; WINDOW * WINDOW * 3];
    for &ty in &tile_starts(height, stride) {
        for &tx in &tile_starts(width, stride) {
            fill_tile_input(image, Vec2us::new(tx, ty), &mut input);
            infer(&input, &mut output);
            for hh in 0..WINDOW {
                for ww in 0..WINDOW {
                    let fw = feather(ww) * feather(hh);
                    let idx = (ty + hh) * width + tx + ww;
                    let s = (hh * WINDOW + ww) * 3;
                    for c in 0..3 {
                        sums[c][idx] += output[s + c] * fw;
                    }
                    weight[idx] += fw;
                }
            }
        }
    }
    if image.is_rgb() {
        sums.iter()
            .map(|sum| {
                sum.iter()
                    .zip(&weight)
                    .map(|(&sum, &wt)| (sum / wt).clamp(0.0, 1.0))
                    .collect()
            })
            .collect()
    } else {
        vec![
            (0..width * height)
                .map(|i| {
                    ((sums[0][i] + sums[1][i] + sums[2][i]) / (3.0 * weight[i])).clamp(0.0, 1.0)
                })
                .collect(),
        ]
    }
}

/// A band of [`WINDOW`] rows blends what the whole frame blended, bit for bit, read in place or
/// written apart: each pixel gathers the same tiles' sums in the same order. A model that squares
/// its input and adds a tenth of the tile's count makes every overlap matter; 600 × 1100 at 384
/// runs tile rows at 0, 384 and 588, so a band row is reused and the last tile row is short.
#[test]
fn a_band_blends_what_the_whole_frame_blended() {
    let size = Size2us::new(600, 1100);
    let rgb = LinearImage::from(array::from_fn::<_, 3, _>(|c| {
        ramp(size.width, size.height, c)
    }));
    let gray = LinearImage::from(ramp(size.width, size.height, 1));
    let model = || {
        let mut count = 0.0f32;
        move |input: &[f32], output: &mut [f32]| {
            count += 1.0;
            for (out, &value) in output.iter_mut().zip(input) {
                *out = value * value + count / 10.0;
            }
        }
    };
    for image in [rgb, gray] {
        let expected = whole_frame_blend(&image, MAX_STRIDE, model());
        let mut in_place = image.clone();
        let mut infer = model();
        run_tiles(
            Planes::InPlace(&mut in_place),
            MAX_STRIDE,
            |input, output| {
                infer(input, output);
                Ok(())
            },
        )
        .unwrap();
        let zeroed = || Buffer2::new_filled(size.width, size.height, 0.0f32);
        let mut apart = if image.is_rgb() {
            LinearImage::from(array::from_fn::<_, 3, _>(|_| zeroed()))
        } else {
            LinearImage::from(zeroed())
        };
        let mut infer = model();
        run_tiles(
            Planes::Apart {
                input: &image,
                output: &mut apart,
            },
            MAX_STRIDE,
            |input, output| {
                infer(input, output);
                Ok(())
            },
        )
        .unwrap();
        for (c, expected) in expected.iter().enumerate() {
            let bits = |plane: &[f32]| {
                plane
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                bits(in_place.channel(c).pixels()),
                bits(expected),
                "channel {c} in place"
            );
            assert_eq!(
                bits(apart.channel(c).pixels()),
                bits(expected),
                "channel {c} apart"
            );
        }
    }
}
