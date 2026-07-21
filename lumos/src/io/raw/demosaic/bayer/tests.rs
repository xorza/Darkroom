//! Tests for Bayer CFA types and RCD demosaicing.

use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern, rcd};
use crate::io::raw::demosaic::interleave_planes;
use common::CancelToken;
use rayon::ThreadPoolBuilder;

#[test]
fn test_cfa_rggb_pattern() {
    let cfa = CfaPattern::Rggb;
    assert_eq!(cfa.color_at(0, 0), 0); // R
    assert_eq!(cfa.color_at(0, 1), 1); // G
    assert_eq!(cfa.color_at(0, 2), 0); // R
    assert_eq!(cfa.color_at(0, 3), 1); // G
    assert_eq!(cfa.color_at(1, 0), 1); // G
    assert_eq!(cfa.color_at(1, 1), 2); // B
    assert_eq!(cfa.color_at(1, 2), 1); // G
    assert_eq!(cfa.color_at(1, 3), 2); // B
}

#[test]
fn test_cfa_bggr_pattern() {
    let cfa = CfaPattern::Bggr;
    assert_eq!(cfa.color_at(0, 0), 2); // B
    assert_eq!(cfa.color_at(0, 1), 1); // G
    assert_eq!(cfa.color_at(1, 0), 1); // G
    assert_eq!(cfa.color_at(1, 1), 0); // R
}

#[test]
fn test_cfa_grbg_pattern() {
    let cfa = CfaPattern::Grbg;
    assert_eq!(cfa.color_at(0, 0), 1); // G
    assert_eq!(cfa.color_at(0, 1), 0); // R
    assert_eq!(cfa.color_at(1, 0), 2); // B
    assert_eq!(cfa.color_at(1, 1), 1); // G
}

#[test]
fn test_cfa_gbrg_pattern() {
    let cfa = CfaPattern::Gbrg;
    assert_eq!(cfa.color_at(0, 0), 1); // G
    assert_eq!(cfa.color_at(0, 1), 2); // B
    assert_eq!(cfa.color_at(1, 0), 0); // R
    assert_eq!(cfa.color_at(1, 1), 1); // G
}

#[test]
fn test_from_bayerpat() {
    assert_eq!(CfaPattern::from_bayerpat("RGGB"), Some(CfaPattern::Rggb));
    assert_eq!(CfaPattern::from_bayerpat("BGGR"), Some(CfaPattern::Bggr));
    assert_eq!(CfaPattern::from_bayerpat("GRBG"), Some(CfaPattern::Grbg));
    assert_eq!(CfaPattern::from_bayerpat("GBRG"), Some(CfaPattern::Gbrg));
    // Case insensitive
    assert_eq!(CfaPattern::from_bayerpat("rggb"), Some(CfaPattern::Rggb));
    // "TRUE" is RGGB
    assert_eq!(CfaPattern::from_bayerpat("TRUE"), Some(CfaPattern::Rggb));
    // Whitespace trimmed
    assert_eq!(CfaPattern::from_bayerpat(" BGGR "), Some(CfaPattern::Bggr));
    // Invalid
    assert_eq!(CfaPattern::from_bayerpat("XXXX"), None);
    assert_eq!(CfaPattern::from_bayerpat(""), None);
}

#[test]
fn test_flip_vertical() {
    // Flip swaps rows: RGGB row0=[R,G] row1=[G,B] → row0=[G,B] row1=[R,G] = GBRG
    assert_eq!(CfaPattern::Rggb.flip_vertical(), CfaPattern::Gbrg);
    assert_eq!(CfaPattern::Gbrg.flip_vertical(), CfaPattern::Rggb);
    assert_eq!(CfaPattern::Bggr.flip_vertical(), CfaPattern::Grbg);
    assert_eq!(CfaPattern::Grbg.flip_vertical(), CfaPattern::Bggr);
    // Double flip is identity
    assert_eq!(
        CfaPattern::Rggb.flip_vertical().flip_vertical(),
        CfaPattern::Rggb
    );
}

#[test]
fn test_flip_horizontal() {
    // Flip swaps columns: RGGB row0=[R,G] row1=[G,B] → row0=[G,R] row1=[B,G] = GRBG
    assert_eq!(CfaPattern::Rggb.flip_horizontal(), CfaPattern::Grbg);
    assert_eq!(CfaPattern::Grbg.flip_horizontal(), CfaPattern::Rggb);
    assert_eq!(CfaPattern::Bggr.flip_horizontal(), CfaPattern::Gbrg);
    assert_eq!(CfaPattern::Gbrg.flip_horizontal(), CfaPattern::Bggr);
    // Double flip is identity
    assert_eq!(
        CfaPattern::Rggb.flip_horizontal().flip_horizontal(),
        CfaPattern::Rggb
    );
}

#[test]
fn test_flip_both_axes() {
    // Flipping both axes is equivalent to 180° rotation
    // RGGB → flip_v → GBRG → flip_h → BGGR
    assert_eq!(
        CfaPattern::Rggb.flip_vertical().flip_horizontal(),
        CfaPattern::Bggr
    );
    assert_eq!(
        CfaPattern::Bggr.flip_vertical().flip_horizontal(),
        CfaPattern::Rggb
    );
}

#[test]
fn raw_origin_pattern_preserves_visible_color_for_every_margin_phase() {
    let visible_patterns = [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ];

    for visible in visible_patterns {
        for top_margin in 0..2 {
            for left_margin in 0..2 {
                let raw = visible.at_raw_origin(top_margin, left_margin);
                for y in 0..4 {
                    for x in 0..4 {
                        assert_eq!(
                            raw.color_at(y + top_margin, x + left_margin),
                            visible.color_at(y, x),
                            "{visible:?}, margin ({top_margin}, {left_margin}), ({y}, {x})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn test_bayer_image_zero_width() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 0, 2, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn test_bayer_image_zero_height() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 0, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Data length")]
fn test_bayer_image_wrong_data_length() {
    let data = vec![0.0f32; 3];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Top margin")]
fn test_bayer_image_margin_exceeds_height() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 1, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Left margin")]
fn test_bayer_image_margin_exceeds_width() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 1, CfaPattern::Rggb);
}

#[test]
fn test_bayer_image_valid() {
    let data = vec![0.0f32; 16];
    let bayer = BayerImage::with_margins(&data, 4, 4, 2, 2, 1, 1, CfaPattern::Rggb);
    assert_eq!(bayer.raw_width, 4);
    assert_eq!(bayer.raw_height, 4);
    assert_eq!(bayer.width, 2);
    assert_eq!(bayer.height, 2);
    assert_eq!(bayer.top_margin, 1);
    assert_eq!(bayer.left_margin, 1);
}

/// Helper: create a BayerImage from a flat CFA array with no margins.
fn make_bayer(data: &[f32], width: usize, height: usize, cfa: CfaPattern) -> BayerImage<'_> {
    BayerImage::with_margins(data, width, height, width, height, 0, 0, cfa)
}

#[test]
fn test_rcd_output_dimensions() {
    // 20x20 image, no margins → output should be 20*20*3 = 1200 floats
    let w = 20;
    let h = 20;
    let data = vec![0.5f32; w * h];
    let bayer = make_bayer(&data, w, h, CfaPattern::Rggb);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());
    assert_eq!(rgb.len(), w * h * 3);
}

#[test]
fn cancelled_token_bails_the_demosaic() {
    let (w, h) = (20, 20);
    let data = vec![0.5f32; w * h];
    let bayer = make_bayer(&data, w, h, CfaPattern::Rggb);

    // A live, tripped token bails at the first between-stage check rather than
    // running the whole demosaic.
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(
        rcd::demosaic(&bayer, &cancel).is_err(),
        "a tripped cancel token must abort the demosaic"
    );

    // An un-cancelled token completes normally.
    assert!(rcd::demosaic(&bayer, &CancelToken::never()).is_ok());
}

#[test]
fn parallel_rcd_matches_single_thread_bit_for_bit() {
    let (w, h) = if cfg!(miri) {
        (20usize, 20usize)
    } else {
        (96usize, 80usize)
    };
    let data: Vec<f32> = (0..w * h)
        .map(|index| ((index * 37 + index / w * 11) % 1_024) as f32 / 1_023.0)
        .collect();
    let bayer = make_bayer(&data, w, h, CfaPattern::Rggb);

    let run = |threads| {
        ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| rcd::demosaic(&bayer, &CancelToken::never()).unwrap())
    };

    let parallel_threads = if cfg!(miri) { 2 } else { 4 };
    assert_eq!(run(1), run(parallel_threads));
}

#[test]
fn test_rcd_uniform_input() {
    // A uniform CFA (all pixels = 0.5) should produce approximately uniform RGB.
    // The ratio correction formula with uniform LPF reduces to:
    //   N_Est = 0.5 * (2*lpf) / (eps + 2*lpf) ≈ 0.5 for large lpf
    // So all channels should be close to 0.5.
    // Use 32x32 with 6-pixel margin to reduce border effects.
    let raw_w = 32;
    let raw_h = 32;
    let val = 0.5f32;
    let data = vec![val; raw_w * raw_h];
    let act_w = 20;
    let act_h = 20;
    let bayer = BayerImage::with_margins(&data, raw_w, raw_h, act_w, act_h, 6, 6, CfaPattern::Rggb);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    // Check all output pixels. With 6 pixels of margin on each side of the raw
    // buffer, even border pixels in the active area have full neighborhoods.
    for y in 0..act_h {
        for x in 0..act_w {
            let idx = (y * act_w + x) * 3;
            let r = rgb[idx];
            let g = rgb[idx + 1];
            let b = rgb[idx + 2];
            // All channels should be very close to 0.5
            assert!(
                (r - val).abs() < 0.01,
                "R at ({x},{y})={r}, expected ~{val}"
            );
            assert!(
                (g - val).abs() < 0.01,
                "G at ({x},{y})={g}, expected ~{val}"
            );
            assert!(
                (b - val).abs() < 0.01,
                "B at ({x},{y})={b}, expected ~{val}"
            );
        }
    }
}

#[test]
fn test_rcd_preserves_cfa_channel() {
    // For a synthetic image, the interpolated value at a known CFA site
    // should preserve the original CFA value for that channel.
    // At an R pixel (0,0) in RGGB, the red channel should equal the CFA value.
    let w = 20;
    let h = 20;
    let mut data = vec![0.3f32; w * h];
    // Set a specific R pixel value
    let ry = 8;
    let rx = 8; // This is R in RGGB (even row, even col)
    assert_eq!(CfaPattern::Rggb.color_at(ry, rx), 0); // confirm it's R
    data[ry * w + rx] = 0.7;

    let bayer = make_bayer(&data, w, h, CfaPattern::Rggb);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    // The red channel at this pixel should be exactly 0.7 (it's the CFA value)
    let out_idx = (ry * w + rx) * 3;
    // Border interpolation doesn't touch interior, so the CFA copy in
    // rgb[0] from the initial loop should survive for R at R-positions.
    // But RCD only overwrites non-native channels. Let's verify:
    // Step 3 writes green at R/B positions → rgb[1] at (8,8) is written
    // Step 4.2 writes missing color at R/B positions → rgb[2] at (8,8) is written
    // rgb[0] at (8,8) is never overwritten → should be 0.7
    assert!(
        (rgb[out_idx] - 0.7).abs() < 1e-6,
        "R at ({rx},{ry})={}, expected 0.7",
        rgb[out_idx]
    );
}

#[test]
fn test_rcd_green_at_red_position_hand_computed() {
    // Build a small 16x16 image where we can hand-compute the green interpolation
    // at a known R position.
    //
    // RGGB pattern at (8,8) = R. Green neighbors are at (8,7), (8,9), (7,8), (9,8).
    // For a uniform image with value V, the LPF at (8,8) is:
    //   lpf = V + 0.5*4V + 0.25*4V = V + 2V + V = 4V
    // Ratio correction: N_Est = cfa[N] * 2*lpf_c / (eps + lpf_c + lpf_N)
    //   = V * 2*4V / (eps + 4V + 4V) = V * 8V / (eps + 8V) ≈ V for large V
    // Since all gradients are equal (uniform image), VH_Disc ≈ 0.5, so:
    //   G = 0.5 * V_est + 0.5 * H_est = V
    //
    // Now test with a non-uniform pattern: green pixels = 0.6, red/blue = 0.3
    let w = 16;
    let h = 16;
    let cfa = CfaPattern::Rggb;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = if cfa.color_at(y, x) == 1 { 0.6 } else { 0.3 };
        }
    }

    let bayer = make_bayer(&data, w, h, cfa);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    // At (8,8) which is R: green should be interpolated from surrounding green values.
    // All green neighbors are 0.6, all same-color neighbors are 0.3.
    // LPF at (8,8): cfa=0.3, cardinal neighbors (all green)=0.6, diagonal=0.3
    //   lpf = 0.3 + 0.5*(0.6+0.6+0.6+0.6) + 0.25*(0.3+0.3+0.3+0.3)
    //       = 0.3 + 1.2 + 0.3 = 1.8
    // LPF at (8,6) (another R position, same pattern): same = 1.8
    // N_Est = cfa[7,8]=0.6 * 2*1.8 / (eps + 1.8 + 1.8) = 0.6 * 3.6/3.6 ≈ 0.6
    // All gradient terms are symmetric → V_est = H_est ≈ 0.6
    // So green at R position ≈ 0.6
    let out_idx = (8 * w + 8) * 3;
    let g_val = rgb[out_idx + 1];
    assert!(
        (g_val - 0.6).abs() < 0.02,
        "Green at R-pos (8,8)={g_val}, expected ~0.6"
    );

    // Also verify the native red channel is preserved
    assert!(
        (rgb[out_idx] - 0.3).abs() < 1e-6,
        "Red at R-pos (8,8)={}, expected 0.3",
        rgb[out_idx]
    );
}

#[test]
fn test_rcd_all_patterns_preserve_native_samples_and_stay_finite() {
    let w = 20;
    let h = 20;
    let data: Vec<f32> = (0..w * h).map(|i| i as f32 / (w * h) as f32).collect();

    for pattern in [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ] {
        let bayer = make_bayer(&data, w, h, pattern);
        let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());
        assert_eq!(rgb.len(), w * h * 3);

        for (sample, &value) in rgb.iter().enumerate() {
            assert!(value.is_finite(), "{pattern:?}: sample {sample} = {value}");
        }
        for y in 0..h {
            for x in 0..w {
                let pixel = y * w + x;
                let channel = pattern.color_at(y, x);
                assert_eq!(
                    rgb[pixel * 3 + channel],
                    data[pixel],
                    "{pattern:?}: native sample changed at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn signed_linear_gradient_crossing_zero_is_reconstructed_without_spikes() {
    let width = 32;
    let height = 24;
    let slope = 0.125;
    let data: Vec<f32> = (0..height)
        .flat_map(|_| (0..width).map(|x| (x as f32 - 15.5) * slope))
        .collect();

    for pattern in [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ] {
        let bayer = make_bayer(&data, width, height, pattern);
        let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

        for y in 6..height - 6 {
            for x in 6..width - 6 {
                let expected = (x as f32 - 15.5) * slope;
                for channel in 0..3 {
                    let actual = rgb[(y * width + x) * 3 + channel];
                    assert!(
                        (actual - expected).abs() < 2e-4,
                        "{pattern:?} channel {channel} at ({x}, {y}): expected {expected}, got {actual}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_rcd_with_margins() {
    let raw_w = 26;
    let raw_h = 26;
    let act_w = 16;
    let act_h = 16;
    let visible_pattern = CfaPattern::Rggb;
    let channel_values = [0.8, 0.5, 0.2];

    for top_margin in 4..6 {
        for left_margin in 4..6 {
            let raw_pattern = visible_pattern.at_raw_origin(top_margin, left_margin);
            let mut data = vec![0.0f32; raw_w * raw_h];
            for y in 0..raw_h {
                for x in 0..raw_w {
                    data[y * raw_w + x] = channel_values[raw_pattern.color_at(y, x)];
                }
            }
            let bayer = BayerImage::with_margins(
                &data,
                raw_w,
                raw_h,
                act_w,
                act_h,
                top_margin,
                left_margin,
                raw_pattern,
            );
            let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());
            assert_eq!(rgb.len(), act_w * act_h * 3);

            for y in 0..act_h {
                for x in 0..act_w {
                    let native_channel = visible_pattern.color_at(y, x);
                    let native = rgb[(y * act_w + x) * 3 + native_channel];
                    assert!(
                        (native - channel_values[native_channel]).abs() < 1e-7,
                        "native margin ({top_margin}, {left_margin}), ({y}, {x})"
                    );
                }
            }

            for y in 5..act_h - 5 {
                for x in 5..act_w - 5 {
                    let index = (y * act_w + x) * 3;
                    for channel in 0..3 {
                        assert!(
                            (rgb[index + channel] - channel_values[channel]).abs() < 0.01,
                            "channel {channel}, margin ({top_margin}, {left_margin}), ({y}, {x})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn test_rcd_gradient_image_green_smoothness() {
    // A horizontal gradient should produce a smooth green channel.
    // No abrupt jumps between adjacent green values in the interior.
    let w = 32;
    let h = 16;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            // Smooth horizontal gradient from 0.1 to 0.9
            data[y * w + x] = 0.1 + 0.8 * (x as f32 / (w - 1) as f32);
        }
    }

    let bayer = make_bayer(&data, w, h, CfaPattern::Rggb);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    // Check that green channel is monotonically increasing (approximately)
    // in the interior rows, allowing small local deviations
    let border = 5;
    for y in border..h - border {
        let mut prev_g = 0.0f32;
        for x in border..w - border {
            let idx = (y * w + x) * 3;
            let g = rgb[idx + 1];
            if x > border {
                // Green should generally increase with x (horizontal gradient).
                // Allow small backward steps due to interpolation, but not large ones.
                assert!(
                    g > prev_g - 0.05,
                    "Green not monotonic at ({x},{y}): {g} < {prev_g} - 0.05"
                );
            }
            prev_g = g;
        }
    }
}

#[test]
fn test_rcd_red_blue_at_green_positions() {
    // Verify R and B interpolation at green CFA positions.
    // Use a pattern where R=0.8 everywhere and B=0.2 everywhere, G=0.5.
    // At green positions, the interpolated R should be ~0.8 and B should be ~0.2.
    let w = 20;
    let h = 20;
    let cfa = CfaPattern::Rggb;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = match cfa.color_at(y, x) {
                0 => 0.8, // R
                1 => 0.5, // G
                2 => 0.2, // B
                _ => unreachable!(),
            };
        }
    }

    let bayer = make_bayer(&data, w, h, cfa);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    // Check interior pixels at green positions
    let border = 5;
    for y in border..h - border {
        for x in border..w - border {
            if cfa.color_at(y, x) != 1 {
                continue; // Only check green CFA positions
            }
            let idx = (y * w + x) * 3;
            let r = rgb[idx];
            let g = rgb[idx + 1];
            let b = rgb[idx + 2];

            // Green should be exactly the CFA value (native channel)
            assert!(
                (g - 0.5).abs() < 1e-6,
                "G at green pos ({x},{y})={g}, expected 0.5"
            );
            // R should be interpolated close to 0.8
            assert!(
                (r - 0.8).abs() < 0.05,
                "R at green pos ({x},{y})={r}, expected ~0.8"
            );
            // B should be interpolated close to 0.2
            assert!(
                (b - 0.2).abs() < 0.05,
                "B at green pos ({x},{y})={b}, expected ~0.2"
            );
        }
    }
}

#[test]
fn test_rcd_blue_at_red_position() {
    // Verify B interpolation at R CFA positions (Step 4.2).
    // Use a pattern where R=0.9, G=0.5, B=0.1.
    // At R positions, blue should be interpolated from diagonal B neighbors.
    let w = 20;
    let h = 20;
    let cfa = CfaPattern::Rggb;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = match cfa.color_at(y, x) {
                0 => 0.9, // R
                1 => 0.5, // G
                2 => 0.1, // B
                _ => unreachable!(),
            };
        }
    }

    let bayer = make_bayer(&data, w, h, cfa);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    let border = 5;
    for y in border..h - border {
        for x in border..w - border {
            if cfa.color_at(y, x) != 0 {
                continue; // Only check R CFA positions
            }
            let idx = (y * w + x) * 3;
            let r = rgb[idx];
            let b = rgb[idx + 2];

            // Red should be the native CFA value
            assert!(
                (r - 0.9).abs() < 1e-6,
                "R at R-pos ({x},{y})={r}, expected 0.9"
            );
            // Blue at R position should be close to 0.1
            assert!(
                (b - 0.1).abs() < 0.05,
                "B at R-pos ({x},{y})={b}, expected ~0.1"
            );
        }
    }
}

#[test]
fn test_rcd_red_at_blue_position() {
    // Step 4.2 reverse: verify R interpolation at B CFA positions.
    // Symmetric to test_rcd_blue_at_red_position but tests the B-row → write-R path.
    // R=0.9, G=0.5, B=0.1. At B positions, R should be interpolated from diagonal R neighbors.
    let w = 20;
    let h = 20;
    let cfa = CfaPattern::Rggb;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = match cfa.color_at(y, x) {
                0 => 0.9,
                1 => 0.5,
                2 => 0.1,
                _ => unreachable!(),
            };
        }
    }

    let bayer = make_bayer(&data, w, h, cfa);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    let border = 5;
    for y in border..h - border {
        for x in border..w - border {
            if cfa.color_at(y, x) != 2 {
                continue; // Only check B CFA positions
            }
            let idx = (y * w + x) * 3;
            let r = rgb[idx];
            let b = rgb[idx + 2];

            // Blue should be the native CFA value
            assert!(
                (b - 0.1).abs() < 1e-6,
                "B at B-pos ({x},{y})={b}, expected 0.1"
            );
            // Red at B position should be close to 0.9 (interpolated from diagonal R neighbors)
            assert!(
                (r - 0.9).abs() < 0.05,
                "R at B-pos ({x},{y})={r}, expected ~0.9"
            );
        }
    }
}

#[test]
fn test_rcd_bggr_correctness() {
    // Verify BGGR pattern produces correct interpolation values, not just valid range.
    // BGGR: (0,0)=B, (0,1)=G, (1,0)=G, (1,1)=R
    // Use distinct per-channel values: R=0.8, G=0.5, B=0.2
    let w = 20;
    let h = 20;
    let cfa = CfaPattern::Bggr;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = match cfa.color_at(y, x) {
                0 => 0.8,
                1 => 0.5,
                2 => 0.2,
                _ => unreachable!(),
            };
        }
    }

    let bayer = make_bayer(&data, w, h, cfa);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    let border = 5;
    for y in border..h - border {
        for x in border..w - border {
            let idx = (y * w + x) * 3;
            let r = rgb[idx];
            let g = rgb[idx + 1];
            let b = rgb[idx + 2];

            // All positions should reconstruct the per-channel values
            assert!(
                (r - 0.8).abs() < 0.05,
                "BGGR R at ({x},{y})={r}, expected ~0.8"
            );
            assert!(
                (g - 0.5).abs() < 0.02,
                "BGGR G at ({x},{y})={g}, expected ~0.5"
            );
            assert!(
                (b - 0.2).abs() < 0.05,
                "BGGR B at ({x},{y})={b}, expected ~0.2"
            );
        }
    }
}

#[test]
fn test_rcd_grbg_gbrg_correctness() {
    // Verify GRBG and GBRG patterns produce correct interpolation.
    // Use R=0.7, G=0.4, B=0.1.
    for cfa in [CfaPattern::Grbg, CfaPattern::Gbrg] {
        let w = 20;
        let h = 20;
        let mut data = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = match cfa.color_at(y, x) {
                    0 => 0.7,
                    1 => 0.4,
                    2 => 0.1,
                    _ => unreachable!(),
                };
            }
        }

        let bayer = make_bayer(&data, w, h, cfa);
        let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

        let border = 5;
        for y in border..h - border {
            for x in border..w - border {
                let idx = (y * w + x) * 3;
                let r = rgb[idx];
                let g = rgb[idx + 1];
                let b = rgb[idx + 2];

                assert!(
                    (r - 0.7).abs() < 0.05,
                    "{cfa:?} R at ({x},{y})={r}, expected ~0.7"
                );
                assert!(
                    (g - 0.4).abs() < 0.02,
                    "{cfa:?} G at ({x},{y})={g}, expected ~0.4"
                );
                assert!(
                    (b - 0.1).abs() < 0.05,
                    "{cfa:?} B at ({x},{y})={b}, expected ~0.1"
                );
            }
        }
    }
}

#[test]
fn test_rcd_vh_direction_sensitivity() {
    // A horizontal edge (intensity change between rows) should produce
    // predominantly vertical interpolation (VH_Dir < 0.5 → favors vertical).
    // A vertical edge (intensity change between columns) should produce
    // predominantly horizontal interpolation (VH_Dir > 0.5 → favors horizontal).
    //
    // We verify this indirectly: a horizontal stripe pattern demosaiced
    // should produce smoother green along rows than columns, and vice versa.
    let w = 32;
    let h = 32;

    // Horizontal stripes: rows 0-15 = 0.8, rows 16-31 = 0.2
    let mut h_data = vec![0.0f32; w * h];
    for y in 0..h {
        let val = if y < h / 2 { 0.8 } else { 0.2 };
        for x in 0..w {
            h_data[y * w + x] = val;
        }
    }
    let h_bayer = make_bayer(&h_data, w, h, CfaPattern::Rggb);
    let h_rgb = interleave_planes(rcd::demosaic(&h_bayer, &CancelToken::never()).unwrap());

    // Vertical stripes: cols 0-15 = 0.8, cols 16-31 = 0.2
    let mut v_data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            v_data[y * w + x] = if x < w / 2 { 0.8 } else { 0.2 };
        }
    }
    let v_bayer = make_bayer(&v_data, w, h, CfaPattern::Rggb);
    let v_rgb = interleave_planes(rcd::demosaic(&v_bayer, &CancelToken::never()).unwrap());

    // For horizontal stripes: green variation along a row (far from edge) should be small
    let test_row = 6; // well inside the uniform top half
    let mut h_row_var = 0.0f32;
    for x in 6..w - 6 - 1 {
        let g0 = h_rgb[(test_row * w + x) * 3 + 1];
        let g1 = h_rgb[(test_row * w + x + 1) * 3 + 1];
        h_row_var += (g1 - g0).abs();
    }

    // For vertical stripes: green variation along a column (far from edge) should be small
    let test_col = 6;
    let mut v_col_var = 0.0f32;
    for y in 6..h - 6 - 1 {
        let g0 = v_rgb[(y * w + test_col) * 3 + 1];
        let g1 = v_rgb[((y + 1) * w + test_col) * 3 + 1];
        v_col_var += (g1 - g0).abs();
    }

    // In uniform regions parallel to the edge, variation should be very small
    // (VH direction correctly detects edge orientation and interpolates along it)
    assert!(
        h_row_var < 0.1,
        "Horizontal stripes: row variation {h_row_var} too high (should be smooth along row)"
    );
    assert!(
        v_col_var < 0.1,
        "Vertical stripes: col variation {v_col_var} too high (should be smooth along column)"
    );
}

#[test]
fn test_rcd_border_interpolation() {
    // Verify that border pixels get reasonable bilinear values, not zeros.
    // Use a uniform image so expected values are known.
    let w = 20;
    let h = 20;
    let val = 0.6f32;
    let data = vec![val; w * h];
    let cfa = CfaPattern::Rggb;
    let bayer = make_bayer(&data, w, h, cfa);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    // Border = 4 pixels. Check the very edge pixels are non-zero and close to val.
    // Corner pixel (0,0) for RGGB is R. Its G and B are bilinear from neighbors.
    // All CFA values are 0.6, so bilinear interpolation → 0.6 for all channels.
    for y in 0..4 {
        for x in 0..w {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                assert!(rgb[idx + c] > 0.0, "Border pixel ({x},{y}) ch {c} is zero");
                assert!(
                    (rgb[idx + c] - val).abs() < 0.15,
                    "Border pixel ({x},{y}) ch {c}={}, expected ~{val}",
                    rgb[idx + c]
                );
            }
        }
    }
    // Bottom border
    for y in h - 4..h {
        for x in 0..w {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                assert!(rgb[idx + c] > 0.0, "Border pixel ({x},{y}) ch {c} is zero");
            }
        }
    }
    // Left/right edges on interior rows
    for y in 4..h - 4 {
        for x in [0, 1, 2, 3, w - 4, w - 3, w - 2, w - 1] {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                assert!(rgb[idx + c] > 0.0, "Border pixel ({x},{y}) ch {c} is zero");
            }
        }
    }
}

#[test]
fn test_rcd_sharp_edge_no_excessive_artifacts() {
    // A sharp vertical edge at column 16: left half = 0.9, right half = 0.1.
    // Verify that the transition zone is bounded (no extreme overshoots from
    // the ratio correction or direction interpolation).
    let w = 32;
    let h = 32;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = if x < 16 { 0.9 } else { 0.1 };
        }
    }

    let bayer = make_bayer(&data, w, h, CfaPattern::Rggb);
    let rgb = interleave_planes(rcd::demosaic(&bayer, &CancelToken::never()).unwrap());

    for (i, &val) in rgb.iter().enumerate() {
        assert!(val.is_finite(), "pixel {i} is non-finite: {val}");
    }

    // Far from edge: left side should be close to 0.9, right side close to 0.1
    let border = 5;
    for y in border..h - border {
        // Well inside left half (col 6-10)
        for x in 6..11 {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                assert!(
                    rgb[idx + c] > 0.7,
                    "Left side ({x},{y}) ch {c}={}, expected >0.7",
                    rgb[idx + c]
                );
            }
        }
        // Well inside right half (col 21-25)
        for x in 21..26 {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                assert!(
                    rgb[idx + c] < 0.3,
                    "Right side ({x},{y}) ch {c}={}, expected <0.3",
                    rgb[idx + c]
                );
            }
        }
    }

    // Transition zone (cols 13-19): values should be monotonically decreasing
    // (approximately) along each row
    for y in border..h - border {
        let mut prev = 1.0f32;
        for x in 13..20 {
            let idx = (y * w + x) * 3;
            let g = rgb[idx + 1]; // check green channel
            // Allow small non-monotonicity from interpolation artifacts
            assert!(
                g < prev + 0.15,
                "Edge transition not bounded at ({x},{y}): g={g}, prev={prev}"
            );
            prev = g;
        }
    }
}
