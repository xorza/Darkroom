use crate::io::raw::demosaic::interleave_planes;
use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::internals::{make_xtrans, test_pattern, to_u16};
use crate::io::raw::demosaic::xtrans::markesteijn_steps::*;

#[test]
fn test_green_minmax_uniform() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; w * h];
    let mut gmax = vec![0.0f32; w * h];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    // Uniform 0.5 input → gmin=gmax≈0.5 everywhere (u16 quantization: ±1e-5)
    for i in 0..w * h {
        assert!((gmin[i] - 0.5).abs() < 1e-4, "gmin[{}] = {}", i, gmin[i]);
        assert!((gmax[i] - 0.5).abs() < 1e-4, "gmax[{}] = {}", i, gmax[i]);
    }
}

#[test]
fn test_green_minmax_bounds() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    // Create gradient data
    let data: Vec<u16> = (0..raw_w * raw_h)
        .map(|i| to_u16((i as f32) / (raw_w * raw_h) as f32))
        .collect();
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; w * h];
    let mut gmax = vec![0.0f32; w * h];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    // gmin should always be <= gmax
    for i in 0..w * h {
        assert!(
            gmin[i] <= gmax[i],
            "gmin[{}] = {} > gmax = {}",
            i,
            gmin[i],
            gmax[i]
        );
    }
}

#[test]
fn test_interpolate_green_uniform() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; w * h];
    let mut gmax = vec![0.0f32; w * h];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    let mut green_dir = vec![0.0f32; NDIR * w * h];
    interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

    // All green values should be 0.5 for uniform input
    for d in 0..NDIR {
        for i in 0..w * h {
            let g = green_dir[d * w * h + i];
            assert!(
                (g - 0.5).abs() < 0.05,
                "green_dir[{}][{}] = {} (expected ~0.5)",
                d,
                i,
                g
            );
        }
    }
}

#[test]
fn test_homogeneity_uniform_derivatives() {
    let w = 12;
    let h = 12;
    let pixels = w * h;

    // Uniform derivatives → all directions equally good
    let drv = vec![1.0f32; NDIR * pixels];
    let mut homo = vec![0u8; NDIR * pixels];
    let mut threshold = vec![0.0f32; pixels];
    compute_homogeneity(&drv, Size2us::new(w, h), &mut homo, &mut threshold);

    // Interior pixels should have equal homogeneity across all directions
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let idx = y * w + x;
            let h0 = homo[idx];
            let h1 = homo[pixels + idx];
            let h2 = homo[2 * pixels + idx];
            let h3 = homo[3 * pixels + idx];
            assert_eq!(h0, h1, "Homogeneity mismatch at ({},{})", y, x);
            assert_eq!(h1, h2, "Homogeneity mismatch at ({},{})", y, x);
            assert_eq!(h2, h3, "Homogeneity mismatch at ({},{})", y, x);
            // With uniform drv=1.0, threshold = 8.0, all drv <= threshold
            // so count should be 9 (full 3×3 window)
            assert_eq!(h0, 9, "Expected 9 at ({},{}), got {}", y, x, h0);
        }
    }
}

#[test]
fn homogeneity_uses_the_center_threshold_for_the_entire_window() {
    let width = 3;
    let height = 3;
    let pixels = width * height;
    let center = 4;
    let mut drv = vec![100.0f32; NDIR * pixels];
    drv[..pixels].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 1.0, 5.0, 6.0, 7.0, 8.0]);
    drv[pixels..2 * pixels].fill(0.1);
    for direction in 1..NDIR {
        drv[direction * pixels + center] = 2.0;
    }
    let mut homo = vec![0u8; NDIR * pixels];
    let mut threshold = vec![0.0f32; pixels];

    compute_homogeneity(&drv, Size2us::new(width, height), &mut homo, &mut threshold);

    assert_eq!(threshold[center], 8.0);
    assert_eq!(homo[center], 9);
}

#[test]
fn test_ypbpr_conversion_white() {
    // White (1,1,1) → Y=1, Pb=0, Pr=0
    let y: f32 = 0.2627 * 1.0 + 0.6780 * 1.0 + 0.0593 * 1.0;
    let pb: f32 = (1.0 - y) * 0.56433;
    let pr: f32 = (1.0 - y) * 0.67815;
    assert!((y - 1.0).abs() < 1e-4, "Y={}", y);
    assert!(pb.abs() < 1e-4, "Pb={}", pb);
    assert!(pr.abs() < 1e-4, "Pr={}", pr);
}

#[test]
fn test_ypbpr_conversion_primary_colors() {
    // Pure red (1,0,0): Y=0.2627, Pb=-0.2627*0.56433, Pr=0.7373*0.67815
    let (y, pb, pr) = rgb_to_ypbpr(1.0, 0.0, 0.0);
    assert!((y - 0.2627).abs() < 1e-6, "Red Y={y}");
    assert!((pb - (-0.2627 * 0.56433)).abs() < 1e-6, "Red Pb={pb}");
    assert!((pr - (0.7373 * 0.67815)).abs() < 1e-4, "Red Pr={pr}");

    // Pure green (0,1,0): Y=0.6780, Pb=-0.6780*0.56433, Pr=-0.6780*0.67815
    let (y, pb, pr) = rgb_to_ypbpr(0.0, 1.0, 0.0);
    assert!((y - 0.6780).abs() < 1e-6, "Green Y={y}");
    assert!((pb - (-0.6780 * 0.56433)).abs() < 1e-6, "Green Pb={pb}");
    assert!((pr - (-0.6780 * 0.67815)).abs() < 1e-4, "Green Pr={pr}");

    // Pure blue (0,0,1): Y=0.0593, Pb=0.9407*0.56433, Pr=-0.0593*0.67815
    let (y, pb, pr) = rgb_to_ypbpr(0.0, 0.0, 1.0);
    assert!((y - 0.0593).abs() < 1e-6, "Blue Y={y}");
    assert!((pb - (0.9407 * 0.56433)).abs() < 1e-4, "Blue Pb={pb}");
    assert!((pr - (-0.0593 * 0.67815)).abs() < 1e-4, "Blue Pr={pr}");

    // Mid-gray (0.5, 0.5, 0.5): Y=0.5, Pb=0, Pr=0
    let (y, pb, pr) = rgb_to_ypbpr(0.5, 0.5, 0.5);
    assert!((y - 0.5).abs() < 1e-6, "Gray Y={y}");
    assert!(pb.abs() < 1e-6, "Gray Pb={pb}");
    assert!(pr.abs() < 1e-6, "Gray Pr={pr}");
}

#[test]
fn test_ypbpr_conversion_black() {
    // Black (0,0,0) → Y=0, Pb=0, Pr=0
    let y: f32 = 0.2627 * 0.0 + 0.6780 * 0.0 + 0.0593 * 0.0;
    let pb: f32 = (0.0 - y) * 0.56433;
    let pr: f32 = (0.0 - y) * 0.67815;
    assert_eq!(y, 0.0);
    assert_eq!(pb, 0.0);
    assert_eq!(pr, 0.0);
}

#[test]
fn test_derivatives_of_uniform_input_are_finite_and_expose_directional_candidates() {
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let pixels = w * h;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; pixels];
    let mut gmax = vec![1.0f32; pixels];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    let mut green_dir = vec![0.0f32; NDIR * pixels];
    interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

    let mut colors = vec![[0.0; 2]; NDIR * pixels];
    reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);
    let mut drv = vec![f32::NAN; NDIR * pixels];
    compute_derivatives(&xtrans, &green_dir, &colors, &mut drv);

    let mut nonzero = [0usize; NDIR];
    for d in 0..NDIR {
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                let val = drv[d * pixels + y * w + x];
                assert!(val.is_finite(), "NaN derivative at d={d} y={y} x={x}");
                assert!(val >= 0.0, "Negative derivative at d={d} y={y} x={x}");
                nonzero[d] += usize::from(val > 1e-6);
            }
        }
    }
    assert!(nonzero[0] > 0);
    assert_ne!(nonzero[0], nonzero[2]);
}

#[test]
fn test_derivatives_checkerboard_nonzero() {
    // Checkerboard input has sharp edges → non-zero Laplacian (derivatives).
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let pixels = w * h;
    let data: Vec<u16> = (0..raw_w * raw_h)
        .map(|i| {
            let y = i / raw_w;
            let x = i % raw_w;
            if (x + y) % 2 == 0 {
                to_u16(0.8)
            } else {
                to_u16(0.2)
            }
        })
        .collect();
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; pixels];
    let mut gmax = vec![1.0f32; pixels];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    let mut green_dir = vec![0.0f32; NDIR * pixels];
    interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

    let mut colors = vec![[0.0; 2]; NDIR * pixels];
    reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);
    let mut drv = vec![0.0f32; NDIR * pixels];
    compute_derivatives(&xtrans, &green_dir, &colors, &mut drv);

    // All derivatives should be finite and non-negative (squared values)
    for (i, &val) in drv.iter().enumerate() {
        assert!(val.is_finite(), "NaN derivative at index {i}");
        assert!(val >= 0.0, "Negative derivative at index {i}: {val}");
    }

    // Checkerboard has high-frequency content → some derivatives must be non-zero
    let mut nonzero_count = 0;
    for d in 0..NDIR {
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                if drv[d * pixels + y * w + x] > 1e-6 {
                    nonzero_count += 1;
                }
            }
        }
    }
    assert!(
        nonzero_count > 0,
        "Expected some non-zero derivatives for checkerboard input"
    );
}

#[test]
fn test_sat_uniform_ones() {
    // 4×3 grid of all 1s
    let data = vec![1u8; 4 * 3];
    let mut sat = vec![u32::MAX; data.len()];
    build_summed_area_table(&data, Size2us::new(4, 3), &mut sat);
    assert_eq!(sat, [1, 2, 3, 4, 2, 4, 6, 8, 3, 6, 9, 12]);

    let origin = Vec2us::ZERO;
    // Full image sum = 12
    assert_eq!(sat_query(&sat, 4, origin, Vec2us::new(3, 2)), 12);
    // Single pixel (0,0) = 1
    assert_eq!(sat_query(&sat, 4, origin, origin), 1);
    // First row sum = 4
    assert_eq!(sat_query(&sat, 4, origin, Vec2us::new(3, 0)), 4);
    // First column sum = 3
    assert_eq!(sat_query(&sat, 4, origin, Vec2us::new(0, 2)), 3);
    // 2×2 top-left corner = 4
    assert_eq!(sat_query(&sat, 4, origin, Vec2us::new(1, 1)), 4);
    // 2×2 bottom-right corner = 4
    assert_eq!(sat_query(&sat, 4, Vec2us::new(2, 1), Vec2us::new(3, 2)), 4);
}

#[test]
fn test_sat_sequential_values() {
    // 3×3 grid: [1,2,3; 4,5,6; 7,8,9]
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
    let mut sat = vec![u32::MAX; data.len()];
    build_summed_area_table(&data, Size2us::new(3, 3), &mut sat);

    // Full sum = 45
    assert_eq!(sat_query(&sat, 3, Vec2us::ZERO, Vec2us::new(2, 2)), 45);
    // Center pixel only = 5
    let center = Vec2us::new(1, 1);
    assert_eq!(sat_query(&sat, 3, center, center), 5);
    // Middle row = 4+5+6 = 15
    assert_eq!(sat_query(&sat, 3, Vec2us::new(0, 1), Vec2us::new(2, 1)), 15);
    // Bottom-right 2×2 = 5+6+8+9 = 28
    assert_eq!(sat_query(&sat, 3, center, Vec2us::new(2, 2)), 28);
}

#[test]
fn test_sat_single_pixel() {
    let data = vec![42u8];
    let mut sat = vec![u32::MAX; data.len()];
    build_summed_area_table(&data, Size2us::new(1, 1), &mut sat);
    assert_eq!(sat_query(&sat, 1, Vec2us::ZERO, Vec2us::ZERO), 42);
}

#[test]
fn test_sat_single_row() {
    let data = vec![1, 2, 3, 4, 5];
    let mut sat = vec![u32::MAX; data.len()];
    build_summed_area_table(&data, Size2us::new(5, 1), &mut sat);
    // Full row = 15
    assert_eq!(sat_query(&sat, 5, Vec2us::ZERO, Vec2us::new(4, 0)), 15);
    // Middle 3 elements = 2+3+4 = 9
    assert_eq!(sat_query(&sat, 5, Vec2us::new(1, 0), Vec2us::new(3, 0)), 9);
}

#[test]
fn test_sat_single_column() {
    let data = vec![1, 2, 3, 4, 5];
    let mut sat = vec![u32::MAX; data.len()];
    build_summed_area_table(&data, Size2us::new(1, 5), &mut sat);
    // Full column = 15
    assert_eq!(sat_query(&sat, 1, Vec2us::ZERO, Vec2us::new(0, 4)), 15);
    // Middle 3 = 2+3+4 = 9
    assert_eq!(sat_query(&sat, 1, Vec2us::new(0, 1), Vec2us::new(0, 3)), 9);
}

#[test]
fn test_sat_zeros() {
    let data = vec![0u8; 4 * 4];
    let mut sat = vec![u32::MAX; data.len()];
    build_summed_area_table(&data, Size2us::new(4, 4), &mut sat);
    assert_eq!(sat_query(&sat, 4, Vec2us::ZERO, Vec2us::new(3, 3)), 0);
}

#[test]
fn homogeneity_scores_match_direct_five_by_five_windows() {
    let width = 6;
    let height = 5;
    let pixels = width * height;
    let mut homo = vec![0u8; NDIR * pixels];
    for direction in 0..NDIR {
        for y in 0..height {
            for x in 0..width {
                homo[direction * pixels + y * width + x] = ((3 * direction + 2 * y + x) % 10) as u8;
            }
        }
    }
    let mut scores = vec![[u32::MAX; NDIR]; pixels];
    let mut sat = vec![u32::MAX; pixels];

    score_homogeneity(&homo, Size2us::new(width, height), &mut scores, &mut sat);

    for direction in 0..NDIR {
        for y in 0..height {
            for x in 0..width {
                let mut expected = 0u32;
                for sample_y in y.saturating_sub(2)..=(y + 2).min(height - 1) {
                    for sample_x in x.saturating_sub(2)..=(x + 2).min(width - 1) {
                        expected += homo[direction * pixels + sample_y * width + sample_x] as u32;
                    }
                }
                assert_eq!(scores[y * width + x][direction], expected);
            }
        }
    }
}

#[test]
fn test_homogeneity_border_pixels_are_zero() {
    let w = 12;
    let h = 12;
    let pixels = w * h;

    let drv = vec![1.0f32; NDIR * pixels];
    let mut homo = vec![0xFFu8; NDIR * pixels]; // fill with garbage to detect missing writes
    let mut threshold = vec![0.0f32; pixels];
    compute_homogeneity(&drv, Size2us::new(w, h), &mut homo, &mut threshold);

    for d in 0..NDIR {
        // Top and bottom rows should be 0
        for x in 0..w {
            assert_eq!(homo[d * pixels + x], 0, "dir={d} top row x={x}");
            assert_eq!(
                homo[d * pixels + (h - 1) * w + x],
                0,
                "dir={d} bottom row x={x}"
            );
        }
        // First and last columns should be 0
        for y in 0..h {
            assert_eq!(homo[d * pixels + y * w], 0, "dir={d} left col y={y}");
            assert_eq!(
                homo[d * pixels + y * w + (w - 1)],
                0,
                "dir={d} right col y={y}"
            );
        }
    }
}

#[test]
fn test_homogeneity_one_dominant_direction() {
    let w = 12;
    let h = 12;
    let pixels = w * h;

    // Direction 0 has very low derivatives, others have high
    let mut drv = vec![100.0f32; NDIR * pixels];
    drv[..pixels].fill(0.1); // dir 0
    let mut homo = vec![0u8; NDIR * pixels];
    let mut threshold = vec![0.0f32; pixels];
    compute_homogeneity(&drv, Size2us::new(w, h), &mut homo, &mut threshold);

    // Interior pixels: dir 0 should have high homogeneity, others low
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let idx = y * w + x;
            let h0 = homo[idx];
            let h1 = homo[pixels + idx];
            assert!(
                h0 > h1,
                "At ({y},{x}): dir0 homo={h0} should be > dir1 homo={h1}"
            );
        }
    }
}

#[test]
fn geometry_stages_cover_each_xtrans_site() {
    let pattern = test_pattern();
    let hex = HexLookup::new(&pattern);
    let mut solitary = 0;
    let mut colored = 0;
    let mut green_block = 0;

    for y in 0..6 {
        for x in 0..6 {
            match pattern.color_at(Vec2us::new(x, y)) {
                0 | 2 => colored += 1,
                1 if is_solitary_green(&hex, y, x) => solitary += 1,
                1 => green_block += 1,
                _ => unreachable!(),
            }
        }
    }

    assert_eq!(solitary, 4);
    assert_eq!(colored, 16);
    assert_eq!(green_block, 16);
}

#[test]
fn reconstruction_preserves_native_samples_and_canonical_empty_directions() {
    let raw_w = 30;
    let raw_h = 30;
    let w = 18;
    let h = 18;
    let pixels = w * h;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);
    let mut gmin = vec![0.0; pixels];
    let mut gmax = vec![0.0; pixels];
    let mut green_dir = vec![0.0; NDIR * pixels];
    let mut colors = vec![[f32::NAN; 2]; NDIR * pixels];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);
    interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);
    reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);

    for direction in 0..NDIR {
        for y in 3..h - 3 {
            for x in 3..w - 3 {
                let raw_y = y + xtrans.margin.y;
                let raw_x = x + xtrans.margin.x;
                let native = xtrans.raw_pattern.color_at(Vec2us::new(raw_x, raw_y));
                let [red, blue] = colors[direction * pixels + y * w + x];
                match native {
                    0 => assert_eq!(red, active_raw(&xtrans, y, x)),
                    2 => assert_eq!(blue, active_raw(&xtrans, y, x)),
                    1 if !is_solitary_green(&hex, raw_y, raw_x) && direction >= 2 => {
                        assert_eq!([red, blue], [0.0, 0.0]);
                        continue;
                    }
                    1 => {}
                    _ => unreachable!(),
                }
                assert!(red.is_finite(), "d={direction} ({y},{x}) R={red}");
                assert!(blue.is_finite(), "d={direction} ({y},{x}) B={blue}");
            }
        }
    }
}

#[test]
fn reconstruction_geometry_dependencies_are_completed_by_earlier_stages() {
    let pattern = test_pattern();
    let hex = HexLookup::new(&pattern);

    for y in 3..9 {
        for x in 3..9 {
            let native = pattern.color_at(Vec2us::new(x, y));
            if native == 1 && !is_solitary_green(&hex, y, x) {
                let offsets = hex.get(y, x);
                for offset in &offsets[..4] {
                    let neighbor_y = y.wrapping_add_signed(offset.dy as isize);
                    let neighbor_x = x.wrapping_add_signed(offset.dx as isize);
                    let neighbor = pattern.color_at(Vec2us::new(neighbor_x, neighbor_y));
                    assert!(
                        neighbor != 1 || is_solitary_green(&hex, neighbor_y, neighbor_x),
                        "2x2-green dependency at ({y},{x}) reaches another block at \
                         ({neighbor_y},{neighbor_x})"
                    );
                }
            }
        }
    }
}

#[test]
fn test_blend_uniform_homo_produces_uniform_output() {
    // With uniform input and uniform homogeneity, output should be uniform
    let raw_w = 24;
    let raw_h = 24;
    let w = 12;
    let h = 12;
    let pixels = w * h;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; pixels];
    let mut gmax = vec![1.0f32; pixels];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    let mut green_dir = vec![0.0f32; NDIR * pixels];
    interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);
    let mut colors = vec![[0.0; 2]; NDIR * pixels];
    reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);

    // Uniform homogeneity → all directions qualify
    let homo = vec![9u8; NDIR * pixels];

    let mut r = vec![0.0f32; pixels];
    let mut g = vec![0.0f32; pixels];
    let mut b = vec![0.0f32; pixels];
    let mut scores = vec![[0; NDIR]; pixels];
    let mut sat = vec![0; pixels];
    blend_final(
        &xtrans,
        &green_dir,
        &colors,
        &homo,
        &mut scores,
        &mut sat,
        &mut r,
        &mut g,
        &mut b,
    );

    // Uniform 0.5 input → output should be approximately 0.5 for all channels
    let output = interleave_planes([r, g, b]);
    for (i, &v) in output.iter().enumerate() {
        assert!((v - 0.5).abs() < 0.05, "Pixel {i}: expected ~0.5, got {v}");
    }
}

#[test]
fn test_blend_one_dominant_direction() {
    // With one dominant direction, output should match that direction's RGB
    let raw_w = 30;
    let raw_h = 30;
    let w = 18;
    let h = 18;
    let pixels = w * h;
    let data = vec![to_u16(0.5); raw_w * raw_h];
    let xtrans = make_xtrans(
        &data,
        Size2us::new(raw_w, raw_h),
        Size2us::new(w, h),
        Vec2us::new(6, 6),
    );
    let hex = HexLookup::new(&xtrans.raw_pattern);

    let mut gmin = vec![0.0f32; pixels];
    let mut gmax = vec![1.0f32; pixels];
    compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

    let mut green_dir = vec![0.0f32; NDIR * pixels];
    interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);
    let mut colors = vec![[0.0; 2]; NDIR * pixels];
    reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);

    // Dir 0 has highest homogeneity (9), others have 0
    let mut homo = vec![0u8; NDIR * pixels];
    homo[..pixels].fill(9);

    let mut r_one = vec![0.0f32; pixels];
    let mut g_one = vec![0.0f32; pixels];
    let mut b_one = vec![0.0f32; pixels];
    let mut scores = vec![[0; NDIR]; pixels];
    let mut sat = vec![0; pixels];
    blend_final(
        &xtrans,
        &green_dir,
        &colors,
        &homo,
        &mut scores,
        &mut sat,
        &mut r_one,
        &mut g_one,
        &mut b_one,
    );
    let output_one = interleave_planes([r_one, g_one, b_one]);

    // All directions equally good
    let homo_all = vec![9u8; NDIR * pixels];
    let mut r_all = vec![0.0f32; pixels];
    let mut g_all = vec![0.0f32; pixels];
    let mut b_all = vec![0.0f32; pixels];
    blend_final(
        &xtrans,
        &green_dir,
        &colors,
        &homo_all,
        &mut scores,
        &mut sat,
        &mut r_all,
        &mut g_all,
        &mut b_all,
    );
    let output_all = interleave_planes([r_all, g_all, b_all]);

    let mut changed = false;
    for y in MARK_INFO_BORDER..h - MARK_INFO_BORDER {
        for x in MARK_INFO_BORDER..w - MARK_INFO_BORDER {
            let pixel = y * w + x;
            let [expected_r, expected_b] = colors[pixel];
            let expected_g = green_dir[pixel];
            assert_eq!(
                &output_one[pixel * 3..pixel * 3 + 3],
                &[expected_r, expected_g, expected_b]
            );
            changed |= output_one[pixel * 3..pixel * 3 + 3] != output_all[pixel * 3..pixel * 3 + 3];
        }
    }
    assert!(changed);

    // Output should have no NaN or negative values
    for (i, &v) in output_one.iter().enumerate() {
        assert!(v.is_finite(), "NaN at {i}");
        assert!(v >= 0.0, "Negative at {i}");
    }
}
