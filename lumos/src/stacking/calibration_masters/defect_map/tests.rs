use crate::io::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::io::image::{ImageDimensions, ImageMetadata};
use crate::stacking::calibration_masters::defect_map::*;
use crate::stacking::combine::cache::{CacheCore, CfaCache};
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::run_stacking;
use crate::stacking::frame_store::{compute_frame_stats, frame_from_memory};
use crate::stacking::progress::ProgressCallback;
use crate::{io::raw::demosaic::bayer::CfaPattern, testing::make_cfa};

#[derive(Debug, PartialEq)]
struct MedianMad {
    median: f32,
    mad: f32,
}

fn median_mad(mut samples: Vec<f32>) -> MedianMad {
    assert!(!samples.is_empty());
    let median = median_f32_mut(&mut samples);
    for sample in &mut samples {
        *sample = (*sample - median).abs();
    }
    MedianMad {
        median,
        mad: median_f32_mut(&mut samples),
    }
}

fn is_hot(defect_map: &DefectMap, pixel_idx: usize) -> bool {
    defect_map.hot_indices.binary_search(&pixel_idx).is_ok()
}

fn is_cold(defect_map: &DefectMap, pixel_idx: usize) -> bool {
    defect_map.cold_indices.binary_search(&pixel_idx).is_ok()
}

#[test]
fn capped_color_sampling_spans_sensor_and_cfa_phases() {
    let cases = [
        (1000, 800, CfaType::Bayer(CfaPattern::Rggb)),
        (1001, 799, CfaType::Bayer(CfaPattern::Bggr)),
        (1003, 797, CfaType::XTrans(XTRANS_PATTERN)),
        (1009, 397, CfaType::Mono),
    ];

    for (width, height, cfa_type) in cases {
        let period = match &cfa_type {
            CfaType::Mono => 1,
            CfaType::Bayer(_) => 2,
            CfaType::XTrans(_) => 6,
        };

        for target_color in 0..cfa_type.num_colors() as u8 {
            let mut expected_rows = vec![false; height];
            let mut expected_columns = vec![false; width];
            let mut expected_phases = vec![false; period * period];
            let mut population = 0;
            for y in 0..height {
                for x in 0..width {
                    if cfa_type.color_at(x, y) == target_color {
                        expected_rows[y] = true;
                        expected_columns[x] = true;
                        expected_phases[(y % period) * period + x % period] = true;
                        population += 1;
                    }
                }
            }

            let indices =
                collect_color_sample_indices(width, height, Some(&cfa_type), target_color);
            assert_eq!(
                indices.len(),
                population.min(MAX_MEDIAN_SAMPLES),
                "{width}x{height} color {target_color} must receive the full per-color cap"
            );
            assert!(
                indices.windows(2).all(|pair| pair[0] < pair[1]),
                "sample indices must be sorted and unique"
            );

            let mut sampled_rows = vec![false; height];
            let mut sampled_columns = vec![false; width];
            let mut sampled_phases = vec![false; period * period];
            for index in indices {
                let x = index % width;
                let y = index / width;
                assert_eq!(cfa_type.color_at(x, y), target_color);
                sampled_rows[y] = true;
                sampled_columns[x] = true;
                sampled_phases[(y % period) * period + x % period] = true;
            }
            assert_eq!(
                sampled_rows, expected_rows,
                "{width}x{height} color {target_color} must span every applicable row"
            );
            assert_eq!(
                sampled_columns, expected_columns,
                "{width}x{height} color {target_color} must span every applicable column"
            );
            assert_eq!(
                sampled_phases, expected_phases,
                "{width}x{height} color {target_color} must span every CFA phase"
            );
        }
    }
}

#[test]
fn capped_color_sampling_matches_exact_row_and_column_statistics() {
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    for (width, height) in [(800, 800), (1000, 800), (800, 1000)] {
        for row_pattern in [true, false] {
            let pixels = (0..width * height)
                .map(|index| {
                    let coordinate = if row_pattern {
                        index / width
                    } else {
                        index % width
                    };
                    (coordinate / 2) as f32
                })
                .collect();
            let data = Buffer2::new(width, height, pixels);
            let exact = median_mad(
                (0..width * height)
                    .filter(|&index| cfa_type.color_at(index % width, index / width) == 0)
                    .map(|index| data[index])
                    .collect(),
            );
            let sampled = median_mad(collect_color_samples(&data, Some(&cfa_type), 0));
            let level_count = if row_pattern { height / 2 } else { width / 2 };
            let pattern_name = if row_pattern { "row" } else { "column" };
            let expected = MedianMad {
                median: (level_count - 1) as f32 * 0.5,
                mad: level_count as f32 * 0.25,
            };

            assert_eq!(
                exact, expected,
                "{width}x{height} {pattern_name} exact statistics must match the hand-computed \
                     uniform-level median and MAD"
            );
            assert_eq!(
                sampled, exact,
                "{width}x{height} {pattern_name} stratification must reproduce exact statistics"
            );
        }
    }
}

#[test]
fn cancelled_detection_returns_error() {
    let image = make_cfa(4, 4, vec![0.5; 16], CfaType::Mono);
    let cancel = CancelToken::new();
    cancel.cancel();

    assert!(matches!(
        DefectMap::default().detect_hot(&image, 5.0, &cancel),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        DefectMap::default().detect_cold(&image, &cancel),
        Err(Error::Cancelled)
    ));
}

#[test]
fn quantization_floor_scales_with_bit_depth_and_master_count() {
    let (width, height) = (128usize, 64usize);
    let expected = [10 * width + 10, 20 * width + 80];
    let below_threshold = [30 * width + 20, 40 * width + 90];

    for bits in [12u32, 14, 16] {
        let quantization_step = 1.0 / ((1u32 << bits) - 1) as f32;
        for frame_count in [1usize, 16, 64] {
            let sigma =
                quantization_step * QUANTIZATION_SIGMA_PER_STEP / (frame_count as f32).sqrt();
            for gain in [0.5f64, 4.0] {
                let mut pixels = vec![0.05f32; width * height];
                for &index in &expected {
                    pixels[index] += 6.0 * sigma;
                }
                for &index in &below_threshold {
                    pixels[index] += 4.0 * sigma;
                }

                let mut dark = make_cfa(width, height, pixels, CfaType::Mono);
                dark.quantization_sigma = Some(sigma);
                dark.metadata.gain = Some(gain);
                let detected = DefectMap::default()
                    .detect_hot(&dark, 5.0, &CancelToken::never())
                    .unwrap()
                    .hot_indices;

                assert_eq!(
                    detected, expected,
                    "{bits}-bit, {frame_count}-frame, gain {gain}: exactly the two 6σ pixels \
                         must pass while both 4σ pixels remain below threshold"
                );
            }
        }
    }
}

#[test]
fn cfa_stack_propagates_raw_quantization_into_hot_detection() {
    let (width, height, frame_count) = (128usize, 64usize, 8usize);
    let dimensions = ImageDimensions::new((width, height), 1);
    let source_sigma = QUANTIZATION_SIGMA_PER_STEP / 4095.0;
    let master_sigma = source_sigma / (frame_count as f32).sqrt();
    let expected = [10 * width + 10, 20 * width + 80];
    let below_threshold = [30 * width + 20, 40 * width + 90];

    let images: Vec<CfaImage> = (0..frame_count)
        .map(|_| {
            let mut pixels = vec![0.05f32; width * height];
            for &index in &expected {
                pixels[index] += 6.0 * master_sigma;
            }
            for &index in &below_threshold {
                pixels[index] += 4.0 * master_sigma;
            }
            let mut image = make_cfa(width, height, pixels, CfaType::Mono);
            image.quantization_sigma = Some(source_sigma);
            image
        })
        .collect();
    let frame_stats = images.iter().map(compute_frame_stats).collect();
    let frames = images.into_iter().map(frame_from_memory).collect();
    let cache = CfaCache {
        frames,
        frame_stats,
        core: CacheCore {
            spill_directory: None,
            dimensions,
            metadata: ImageMetadata {
                cfa_type: Some(CfaType::Mono),
                ..Default::default()
            },
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
        },
    };

    let master = run_stacking(&cache, &StackConfig::default());
    assert!(
        (master.quantization_sigma.unwrap() - master_sigma).abs() < f32::EPSILON,
        "eight equal surviving frames must propagate σ/√8"
    );
    let detected = DefectMap::default()
        .detect_hot(&master, 5.0, &CancelToken::never())
        .unwrap()
        .hot_indices;
    assert_eq!(
        detected, expected,
        "the stacked CFA floor must retain both 6σ pixels and reject both 4σ pixels"
    );
}

#[test]
fn test_correct_clustered_defect_uses_only_good_neighbors() {
    // A defect whose same-color neighbours are MOSTLY other defects must still be repaired from
    // the few good ones — the defect mask excludes the bad neighbours. Pre-mask, the neighbour
    // median was dominated by the cluster and left the pixel ~uncorrected (≈0.95 here).
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    let (w, h) = (16usize, 16usize);
    // Red pixels sit at (even,even). Target (6,6); make 5 of its 8 stride-2 red neighbours hot.
    let hot = [(6, 6), (4, 6), (8, 6), (6, 4), (6, 8), (4, 4)];
    let mut px = vec![0.5f32; w * h];
    for &(x, y) in &hot {
        px[y * w + x] = 0.95;
    }
    let dark = make_cfa(w, h, px, cfa.clone());
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();
    assert_eq!(defect_map.hot_count(), 6, "all six 0.95 red pixels are hot");

    let mut light = make_cfa(w, h, vec![0.5f32; w * h], cfa);
    for &(x, y) in &hot {
        light.data[y * w + x] = 0.95;
    }
    defect_map.correct(&mut light);

    // (6,6)'s only good red neighbours (4,8),(8,4),(8,8) are all 0.5 → median 0.5, despite the
    // five hot neighbours that would otherwise dominate.
    let corrected = light.data[6 * w + 6];
    assert!(
        (corrected - 0.5).abs() < 1e-4,
        "clustered defect repaired from good neighbours → expected 0.5, got {corrected}"
    );
}

#[test]
fn test_xtrans_hot_pixel_correction_uses_same_color() {
    // X-Trans hot pixels must be repaired from SAME-COLOR neighbours, not the global mean.
    let pattern = [
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ];
    let cfa = CfaType::XTrans(pattern);
    let (w, h) = (12usize, 12usize);
    // Distinct per-color baselines so a wrong-color repair is detectable.
    let color_val = |c: u8| match c {
        0 => 0.1, // R
        1 => 0.2, // G
        _ => 0.3, // B
    };
    let build = |corrupt: &[(usize, usize)]| {
        let mut px = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] = color_val(cfa.color_at(x, y));
            }
        }
        for &(x, y) in corrupt {
            px[y * w + x] = 0.9;
        }
        make_cfa(w, h, px, cfa.clone())
    };

    let r_hot = (1usize, 0usize); // pattern[0][1] = 0 → R
    let b_hot = (0usize, 1usize); // pattern[1][0] = 2 → B
    assert_eq!(cfa.color_at(r_hot.0, r_hot.1), 0);
    assert_eq!(cfa.color_at(b_hot.0, b_hot.1), 2);

    let dark = build(&[r_hot, b_hot]);
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();
    assert_eq!(defect_map.hot_count(), 2, "one R and one B hot pixel");

    let mut light = build(&[r_hot, b_hot]);
    defect_map.correct(&mut light);

    let r_val = light.data[r_hot.1 * w + r_hot.0];
    let b_val = light.data[b_hot.1 * w + b_hot.0];
    assert!(
        (r_val - 0.1).abs() < 1e-4,
        "R hot repaired from R neighbours → expected 0.1, got {r_val}"
    );
    assert!(
        (b_val - 0.3).abs() < 1e-4,
        "B hot repaired from B neighbours → expected 0.3, got {b_val}"
    );
}

#[test]
fn test_cfa_hot_pixel_detection() {
    // 6x6 CFA image with known hot pixels
    let mut pixels = vec![100.0; 36];
    pixels[0] = 10000.0; // hot at (0,0)
    pixels[14] = 10000.0; // hot at (2,2)
    pixels[35] = 10000.0; // hot at (5,5)

    let dark = make_cfa(6, 6, pixels, CfaType::Bayer(CfaPattern::Rggb));
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();

    assert_eq!(defect_map.hot_count(), 3);
    assert!(is_hot(&defect_map, 0));
    assert!(is_hot(&defect_map, 14));
    assert!(is_hot(&defect_map, 35));
    assert!(!is_hot(&defect_map, 1)); // not hot
}

#[test]
fn test_cfa_hot_pixel_correction_bayer() {
    // 6x6 Bayer RGGB pattern
    // Hot pixel at (2,2) = R. Same-color (R) neighbors at stride 2.
    let mut pixels = vec![100.0; 36];
    pixels[2 * 6 + 2] = 10000.0; // hot at (2,2)

    let mut image = make_cfa(6, 6, pixels, CfaType::Bayer(CfaPattern::Rggb));

    let defect_map = DefectMap {
        hot_indices: vec![2 * 6 + 2],
        cold_indices: vec![],
        dimensions: Some(Vec2us::new(6, 6)),
    };

    defect_map.correct(&mut image);

    // Should be replaced with median of same-color neighbors (all 100.0)
    assert!(
        (image.data[2 * 6 + 2] - 100.0).abs() < f32::EPSILON,
        "Expected 100.0, got {}",
        image.data[2 * 6 + 2]
    );
}

#[test]
fn test_cfa_hot_pixel_correction_mono() {
    // Mono: uses standard 8-connected neighbors
    let pixels = vec![10.0, 20.0, 30.0, 40.0, 1000.0, 50.0, 60.0, 70.0, 80.0];
    let mut image = make_cfa(3, 3, pixels, CfaType::Mono);

    let defect_map = DefectMap {
        hot_indices: vec![4],
        cold_indices: vec![],
        dimensions: Some(Vec2us::new(3, 3)),
    };

    defect_map.correct(&mut image);

    // Median of [10, 20, 30, 40, 50, 60, 70, 80] = 45
    assert!(
        (image.data[4] - 45.0).abs() < f32::EPSILON,
        "Expected 45.0, got {}",
        image.data[4]
    );
}

#[test]
fn test_bayer_same_color_neighbors() {
    // 6x6 image, all 100.0, hot pixel at center (2,2)
    let mut pixels = vec![100.0; 36];
    // Set some same-color neighbors to distinct values to verify median
    pixels[0] = 50.0; // (0,0)
    pixels[4] = 60.0; // (4,0)
    pixels[2] = 70.0; // (2,0)
    pixels[4 * 6 + 2] = 80.0; // (2,4)

    let pixels = imaginarium::Buffer2::new(6, 6, pixels);
    let result = bayer_same_color_median(&pixels, 2, 2, None);

    // Neighbors: 50, 60, 70, 80, 100 (0,2=100), 100 (4,2=100), 100 (0,4=100), 100 (4,4=100)
    // Sorted: 50, 60, 70, 80, 100, 100, 100, 100 → median of 8 = (80+100)/2 = 90
    assert!(
        (result - 90.0).abs() < f32::EPSILON,
        "Expected 90.0, got {}",
        result
    );
}

#[test]
fn test_bayer_same_color_neighbors_corner() {
    // Hot pixel at corner (0,0) in 4x4 Bayer RGGB
    // Same-color (R) neighbors at stride 2: (2,0), (0,2), (2,2)
    let pixels = vec![
        999.0, 10.0, 50.0, 10.0, 10.0, 10.0, 10.0, 10.0, 60.0, 10.0, 70.0, 10.0, 10.0, 10.0, 10.0,
        10.0,
    ];
    let pixels = imaginarium::Buffer2::new(4, 4, pixels);
    let result = bayer_same_color_median(&pixels, 0, 0, None);

    // Same-color neighbors: (2,0)=50, (0,2)=60, (2,2)=70
    // Median of [50, 60, 70] = 60
    assert!(
        (result - 60.0).abs() < f32::EPSILON,
        "Expected 60.0, got {}",
        result
    );
}

#[test]
fn test_cfa_hot_pixel_detection_large() {
    // Large enough to trigger sampling
    let size = 500;
    let pixel_count = size * size;
    let mut pixels = vec![100.0; pixel_count];

    let hot_positions = [0, 500, 5000, 50000, 100000, 200000, 249999];
    for &idx in &hot_positions {
        pixels[idx] = 10000.0;
    }

    let dark = make_cfa(size, size, pixels, CfaType::Bayer(CfaPattern::Rggb));
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();

    assert_eq!(defect_map.hot_count(), hot_positions.len());
    for &idx in &hot_positions {
        assert!(
            is_hot(&defect_map, idx),
            "Hot pixel at {} not detected",
            idx
        );
    }
}

#[test]
fn test_per_channel_detection_bayer() {
    // 8x8 Bayer RGGB image.
    // Red pixels (at even x, even y) have value 100.0
    // Green pixels have value 200.0
    // Blue pixels (at odd x, odd y) have value 50.0
    // One red pixel is hot at 500.0 — should be detected by per-channel stats
    // even though 500 might not exceed a global threshold dominated by green=200.
    let pattern = CfaType::Bayer(CfaPattern::Rggb);
    let mut pixels = vec![0.0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let color = pattern.color_at(x, y);
            pixels[y * 8 + x] = match color {
                0 => 100.0, // R
                1 => 200.0, // G
                2 => 50.0,  // B
                _ => unreachable!(),
            };
        }
    }
    // Make one red pixel hot
    pixels[0] = 500.0; // (0,0) = R

    let dark = make_cfa(8, 8, pixels, pattern.clone());
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 3.0, &CancelToken::never())
        .unwrap();

    // The hot red pixel should be detected
    assert!(
        is_hot(&defect_map, 0),
        "Hot red pixel at (0,0) not detected"
    );

    // Green and blue pixels should not be flagged
    assert!(!is_hot(&defect_map, 1)); // G at (1,0)
    assert!(!is_hot(&defect_map, 9)); // B at (1,1)
}

#[test]
fn dark_background_reconstructs_affine_mono_signal_through_image_edges() {
    let (width, height) = (192usize, 128usize);
    let pixels: Vec<f32> = (0..width * height)
        .map(|index| {
            let x = (index % width) as f32;
            let y = (index / width) as f32;
            0.02 + 0.0001 * x + 0.0002 * y
        })
        .collect();
    let data = Buffer2::new(width, height, pixels);
    let background =
        DarkBackground::fit(&data, Some(&CfaType::Mono), &CancelToken::never()).unwrap();

    for y in 0..height {
        for x in 0..width {
            let expected = 0.02 + 0.0001 * x as f32 + 0.0002 * y as f32;
            assert!(
                (background.at(x, y, 0) - expected).abs() < 2e-7,
                "affine background mismatch at ({x}, {y}): expected {expected}, got {}",
                background.at(x, y, 0)
            );
        }
    }
}

#[test]
fn hot_detection_rejects_column_noise_gradient_and_amp_glow_but_keeps_clusters() {
    let (width, height) = (384usize, 256usize);
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    let isolated = [(17usize, 23usize), (201, 48), (312, 190)];
    let mut cluster = Vec::new();
    for y in (124..=132).step_by(2) {
        for x in (156..=164).step_by(2) {
            assert_eq!(cfa.color_at(x, y), 0);
            cluster.push((x, y));
        }
    }

    let mut pixels = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let color = cfa.color_at(x, y) as usize;
            let x_unit = x as f32 / (width - 1) as f32;
            let y_unit = y as f32 / (height - 1) as f32;
            let baseline = [0.01, 0.02, 0.03][color];
            let gradient = [0.012, 0.018, 0.024][color] * x_unit + 0.01 * y_unit;
            let amp_glow = [0.05, 0.07, 0.09][color] * x_unit * x_unit * (0.5 + 0.5 * y_unit);
            let column_noise = ((x * 13) % 11) as f32 * 0.00004 - 0.0002;
            let noise = ((x * 37 + y * 19) % 17) as f32 * 0.00003 - 0.00024;
            pixels[y * width + x] = baseline + gradient + amp_glow + column_noise + noise;
        }
    }

    let mut expected: Vec<usize> = isolated
        .iter()
        .chain(&cluster)
        .map(|&(x, y)| y * width + x)
        .collect();
    expected.sort_unstable();
    for &index in &expected {
        pixels[index] += 0.08;
    }

    let dark = make_cfa(width, height, pixels, cfa);
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();

    assert_eq!(
        defect_map.hot_indices, expected,
        "smooth per-color structure must not become defects, while every injected point and \
             same-color cluster member must remain detectable"
    );
}

#[test]
fn test_cfa_no_defective_pixels() {
    let pixels = vec![100.0; 36];
    let dark = make_cfa(6, 6, pixels, CfaType::Mono);
    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();
    assert_eq!(defect_map.hot_count(), 0);
    assert_eq!(defect_map.count(), 0);
}

/// Regression for the negative-median-dark bug: a master dark centered near zero (with a
/// negative noise tail, as after bias subtraction) must NOT flag the sub-zero pixels as
/// defects. Only the handful of genuinely hot pixels should be flagged (≪1%).
#[test]
fn near_zero_median_dark_flags_only_hot_pixels() {
    // 64x64 mono dark: small zero-mean noise (many pixels < 0), median ≈ 0.
    let (w, h) = (64usize, 64usize);
    let mut pixels: Vec<f32> = (0..w * h)
        .map(|i| if i % 2 == 0 { -0.0003 } else { 0.0002 })
        .collect();
    // Three genuinely hot pixels.
    for &idx in &[100usize, 2000, 4000] {
        pixels[idx] = 0.5;
    }
    let dark = make_cfa(w, h, pixels, CfaType::Mono);

    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();

    // Exactly the hot pixels — the ~half-the-frame sub-zero pixels are not "cold defects".
    assert_eq!(
        defect_map.count(),
        3,
        "only the hot pixels should be flagged"
    );
    assert!(is_hot(&defect_map, 100));
    assert!(is_hot(&defect_map, 2000));
    assert!(is_hot(&defect_map, 4000));
    assert!(
        defect_map.percentage() < 1.0,
        "defects should be ≪1%, got {:.2}%",
        defect_map.percentage()
    );
}

/// Cold/dead pixels come from the *flat* (illuminated), where a dead pixel is a dark spot.
#[test]
fn cold_pixels_detected_from_flat() {
    // 6x6 mono flat: uniform illumination 0.4 with one dead pixel (no response).
    let mut pixels = vec![0.4f32; 36];
    pixels[5] = 0.0; // dead pixel at index 5
    let flat = make_cfa(6, 6, pixels, CfaType::Mono);

    let defect_map = DefectMap::default()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();

    // The dead pixel (0.0) reads below half its uniform 0.4 neighbourhood (0.5·0.4 = 0.2);
    // every normal pixel reads its full 0.4. A flat yields no hot pixels.
    assert_eq!(defect_map.cold_count(), 1, "the dead pixel is cold");
    assert_eq!(defect_map.hot_count(), 0, "a flat yields no hot pixels");
    assert!(is_cold(&defect_map, 5));
    assert!(!is_cold(&defect_map, 0)); // a normal illuminated pixel
}

/// A clean (uniform) flat must not flag its own pixels as cold — every pixel equals its
/// neighbourhood median, so none falls below half of it.
#[test]
fn uniform_flat_flags_no_cold() {
    let flat = make_cfa(8, 8, vec![0.5f32; 64], CfaType::Mono);
    let defect_map = DefectMap::default()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();
    assert_eq!(defect_map.count(), 0);
}

/// The point of the local test: cold detection must survive a vignetted flat. A steep
/// illumination gradient (0.2 → 0.8 across the frame, mimicking vignetting) defeats any
/// *global* cut — `median − 5σ` goes negative (under-detects), while `0.5 × global_median =
/// 0.25` wrongly flags the whole dim edge (0.2 < 0.25). The local-neighbour ratio flags only
/// the genuinely dead pixel and leaves both edges alone.
#[test]
fn cold_detection_survives_vignetting_gradient() {
    let (w, h) = (16usize, 16usize);
    let mut pixels: Vec<f32> = (0..w * h)
        .map(|i| 0.2 + 0.6 * (i % w) as f32 / (w - 1) as f32)
        .collect();
    for y in 6..10 {
        for x in 2..6 {
            pixels[y * w + x] *= 0.65;
        }
    }
    let dead = 8 * w + 8; // (8,8), normally ≈0.52; its 8 neighbours median ≈0.52
    pixels[dead] = 0.0;
    let flat = make_cfa(w, h, pixels, CfaType::Mono);

    let defect_map = DefectMap::default()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();

    assert_eq!(defect_map.cold_count(), 1, "only the dead pixel is cold");
    assert_eq!(defect_map.hot_count(), 0, "a flat yields no hot pixels");
    assert!(is_cold(&defect_map, dead));
    assert!(
        !is_cold(&defect_map, 8 * w),
        "dim edge (0.2) is vignetting, not dead"
    );
    assert!(
        !is_cold(&defect_map, 8 * w + 15),
        "bright edge (0.8) is fine"
    );
    for y in 6..10 {
        for x in 2..6 {
            assert!(
                !is_cold(&defect_map, y * w + x),
                "dust shadow ({x}, {y}) is attenuated, not dead"
            );
        }
    }
}

/// `detect_hot` + `detect_cold` combine hot pixels (from the dark) and cold pixels (from the
/// flat) into one map.
#[test]
fn detect_hot_and_cold_combine() {
    // Near-zero dark with one hot pixel; illuminated flat with one dead pixel.
    let mut dark_px = vec![0.001f32; 36];
    dark_px[0] = 0.5; // hot
    let dark = make_cfa(6, 6, dark_px, CfaType::Mono);

    let mut flat_px = vec![0.4f32; 36];
    flat_px[5] = 0.0; // dead
    let flat = make_cfa(6, 6, flat_px, CfaType::Mono);

    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();

    assert_eq!(defect_map.hot_count(), 1);
    assert_eq!(defect_map.cold_count(), 1);
    assert_eq!(defect_map.count(), 2);
    assert!(is_hot(&defect_map, 0));
    assert!(is_cold(&defect_map, 5));
}

/// A representative non-trivial X-Trans pattern (R=0, G=1, B=2) reused by the X-Trans tests.
const XTRANS_PATTERN: [[u8; 6]; 6] = [
    [1, 0, 1, 1, 2, 1],
    [2, 1, 2, 0, 1, 0],
    [1, 2, 1, 1, 0, 1],
    [1, 2, 1, 1, 0, 1],
    [0, 1, 0, 2, 1, 2],
    [1, 0, 1, 1, 2, 1],
];

/// Reference X-Trans same-color median: collect every in-bounds, unmasked same-color neighbour
/// in the radius-6 window, take the closest `XTRANS_NEIGHBORS` by Manhattan distance (ties in
/// scan order), median them. The precomputed [`XTransOffsets`] must reproduce this exactly.
fn brute_force_xtrans_median(pixels: &Buffer2<f32>, x: usize, y: usize, pattern: &CfaType) -> f32 {
    let (w, h) = (pixels.width() as i32, pixels.height() as i32);
    let my_color = pattern.color_at(x, y);
    let mut cands: Vec<(i32, f32)> = Vec::new();
    for dy in -XTRANS_RADIUS..=XTRANS_RADIUS {
        for dx in -XTRANS_RADIUS..=XTRANS_RADIUS {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            if pattern.color_at(nx as usize, ny as usize) == my_color {
                cands.push((dx.abs() + dy.abs(), *pixels.get(nx as usize, ny as usize)));
            }
        }
    }
    cands.sort_by_key(|&(dist, _)| dist);
    let n = cands.len().min(XTRANS_NEIGHBORS);
    let mut vals: Vec<f32> = cands[..n].iter().map(|&(_, v)| v).collect();
    median_f32_mut(&mut vals)
}

/// The precomputed X-Trans offsets must reproduce the brute-force closest-N same-color median at
/// every pixel — including borders (fewer neighbours) and interior (the N-cutoff is exercised).
#[test]
fn xtrans_offsets_match_brute_force() {
    let pattern = CfaType::XTrans(XTRANS_PATTERN);
    let (w, h) = (29usize, 23usize); // not a multiple of 6, so all 36 phases hit the borders
    // Deterministic, well-spread values so medians are sensitive to which neighbours are chosen.
    let px: Vec<f32> = (0..w * h)
        .map(|i| ((i.wrapping_mul(2_654_435_761) >> 8) % 1000) as f32 / 1000.0)
        .collect();
    let pixels = Buffer2::new(w, h, px);
    let offsets = XTransOffsets::new(&XTRANS_PATTERN);

    for y in 0..h {
        for x in 0..w {
            let got = offsets.median(&pixels, x, y, None);
            let want = brute_force_xtrans_median(&pixels, x, y, &pattern);
            assert_eq!(
                got, want,
                "X-Trans median mismatch at ({x},{y}): precomputed {got} vs brute-force {want}"
            );
        }
    }
}

/// X-Trans same-color selection: with each color held at a distinct constant, an interior
/// pixel's same-color median is exactly its own color's value (a wrong-color pick would mix them).
#[test]
fn xtrans_median_selects_same_color() {
    let pattern = CfaType::XTrans(XTRANS_PATTERN);
    let (w, h) = (24usize, 24usize);
    let color_val = |c: u8| 0.1 * (c + 1) as f32; // R→0.1, G→0.2, B→0.3
    let px: Vec<f32> = (0..w * h)
        .map(|i| color_val(pattern.color_at(i % w, i / w)))
        .collect();
    let pixels = Buffer2::new(w, h, px);
    let neighbors = SameColorMedian::new(Some(&pattern));

    // Interior pixels (≥6 from every border) of each color — all 24 nearest same-color in-bounds.
    for &(x, y) in &[(13usize, 12usize), (12, 12), (14, 13)] {
        let c = pattern.color_at(x, y);
        let got = neighbors.at(&pixels, x, y, None);
        assert!(
            (got - color_val(c)).abs() < f32::EPSILON,
            "({x},{y}) color {c}: expected {} got {got}",
            color_val(c)
        );
    }
}

/// X-Trans cold detection: a dead pixel reads below half its same-color neighbourhood and is the
/// only one flagged; a normal pixel (value == its neighbourhood median) is not.
#[test]
fn xtrans_cold_pixel_detected() {
    let pattern = CfaType::XTrans(XTRANS_PATTERN);
    let (w, h) = (24usize, 24usize);
    let color_val = |c: u8| 0.1 * (c + 1) as f32;
    let mut px: Vec<f32> = (0..w * h)
        .map(|i| color_val(pattern.color_at(i % w, i / w)))
        .collect();
    let dead = 12 * w + 12; // interior G pixel: 0.0 < 0.5 · 0.2 neighbourhood median
    assert_eq!(pattern.color_at(12, 12), 1, "(12,12) is green");
    px[dead] = 0.0;
    let flat = make_cfa(w, h, px, pattern);

    let defect_map = DefectMap::default()
        .detect_cold(&flat, &CancelToken::never())
        .unwrap();

    assert_eq!(defect_map.cold_count(), 1, "only the dead pixel is cold");
    assert_eq!(defect_map.hot_count(), 0, "detect_cold sets no hot pixels");
    assert!(is_cold(&defect_map, dead));
    // A normal R neighbour: one dead neighbour can't drag its 24-sample median below half.
    assert!(!is_cold(&defect_map, 12 * w + 13));
}

#[test]
#[should_panic(expected = "don't match")]
fn test_correct_cfa_dimension_mismatch() {
    let pixels = vec![10.0; 9];
    let mut image = make_cfa(3, 3, pixels, CfaType::Mono);

    let defect_map = DefectMap {
        hot_indices: vec![],
        cold_indices: vec![],
        dimensions: Some(Vec2us::new(2, 2)),
    };

    defect_map.correct(&mut image);
}
