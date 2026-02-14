//! Tests for Bayer CFA types and RCD demosaicing.

use super::{BayerImage, CfaPattern, demosaic_bayer};

// ── CFA pattern tests ────────────────────────────────────────

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
fn test_red_in_row() {
    assert!(CfaPattern::Rggb.red_in_row(0));
    assert!(!CfaPattern::Rggb.red_in_row(1));
    assert!(CfaPattern::Rggb.red_in_row(2));

    assert!(!CfaPattern::Bggr.red_in_row(0));
    assert!(CfaPattern::Bggr.red_in_row(1));
    assert!(!CfaPattern::Bggr.red_in_row(2));

    assert!(CfaPattern::Grbg.red_in_row(0));
    assert!(!CfaPattern::Grbg.red_in_row(1));

    assert!(!CfaPattern::Gbrg.red_in_row(0));
    assert!(CfaPattern::Gbrg.red_in_row(1));
}

#[test]
fn test_pattern_2x2() {
    assert_eq!(CfaPattern::Rggb.pattern_2x2(), [0, 1, 1, 2]);
    assert_eq!(CfaPattern::Bggr.pattern_2x2(), [2, 1, 1, 0]);
    assert_eq!(CfaPattern::Grbg.pattern_2x2(), [1, 0, 2, 1]);
    assert_eq!(CfaPattern::Gbrg.pattern_2x2(), [1, 2, 0, 1]);
}

// ── from_bayerpat tests ──────────────────────────────────────

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

// ── flip tests ───────────────────────────────────────────────

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

// ── BayerImage validation tests ──────────────────────────────

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

// ── RCD demosaic tests ───────────────────────────────────────

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
    let rgb = demosaic_bayer(&bayer);
    assert_eq!(rgb.len(), w * h * 3);
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
    let rgb = demosaic_bayer(&bayer);

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
    let rgb = demosaic_bayer(&bayer);

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
    let rgb = demosaic_bayer(&bayer);

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
fn test_rcd_all_patterns_produce_valid_output() {
    // All 4 CFA patterns should produce valid (non-NaN, in-range) output.
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
        let rgb = demosaic_bayer(&bayer);
        assert_eq!(rgb.len(), w * h * 3);

        for (i, &val) in rgb.iter().enumerate() {
            assert!(
                val.is_finite() && (0.0..=1.0).contains(&val),
                "Pattern {:?}: pixel {} = {} (out of range)",
                pattern,
                i,
                val
            );
        }
    }
}

#[test]
fn test_rcd_with_margins() {
    // Test that margins are handled correctly.
    // Raw buffer is 24x24, active area is 16x16 starting at (4,4).
    let raw_w = 24;
    let raw_h = 24;
    let act_w = 16;
    let act_h = 16;
    let tm = 4;
    let lm = 4;

    let data = vec![0.4f32; raw_w * raw_h];
    let bayer =
        BayerImage::with_margins(&data, raw_w, raw_h, act_w, act_h, tm, lm, CfaPattern::Rggb);
    let rgb = demosaic_bayer(&bayer);

    // Output should be active area size
    assert_eq!(rgb.len(), act_w * act_h * 3);

    // Uniform input → uniform output
    let border = 5;
    for y in border..act_h - border {
        for x in border..act_w - border {
            let idx = (y * act_w + x) * 3;
            for c in 0..3 {
                assert!(
                    (rgb[idx + c] - 0.4).abs() < 0.01,
                    "ch {} at ({x},{y})={}, expected ~0.4",
                    c,
                    rgb[idx + c]
                );
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
    let rgb = demosaic_bayer(&bayer);

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
    let rgb = demosaic_bayer(&bayer);

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
    let rgb = demosaic_bayer(&bayer);

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
