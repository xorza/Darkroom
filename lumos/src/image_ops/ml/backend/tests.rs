use crate::image_ops::ml::backend::*;

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
    let plane = Buffer2::new(
        side,
        side,
        (0..side * side)
            .map(|i| match i {
                0 => -0.25,
                1 => 1.5,
                _ => 0.5,
            })
            .collect(),
    );
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
/// Gray averages its three equal channels first, two sums and a product more: 8ε bounds both.
#[test]
fn an_identity_model_reproduces_the_input() {
    let size = Size2us::new(700, 600);
    let rgb = LinearImage::from(array::from_fn::<_, 3, _>(|c| {
        ramp(size.width, size.height, c)
    }));
    let gray = LinearImage::from(ramp(size.width, size.height, 1));
    for image in [rgb, gray] {
        let pixels = size.pixel_count();
        let mut acc = [vec![0.0f32; pixels], vec![0.0; pixels], vec![0.0; pixels]];
        let mut weight = vec![0.0f32; pixels];
        let mut input = vec![0.0f32; WINDOW * WINDOW * 3];
        for &ty in &tile_starts(size.height, 256) {
            for &tx in &tile_starts(size.width, 256) {
                let tile = Vec2us::new(tx, ty);
                fill_tile_input(&image, tile, &mut input);
                accumulate(&input, tile, size.width, &mut acc, &mut weight);
            }
        }
        let output = build_output(image.is_rgb(), &acc, &weight, size);
        assert_eq!(output.channels(), image.channels());
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
