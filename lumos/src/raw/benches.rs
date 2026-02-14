use std::time::Instant;

use bench::quick_bench;

use super::*;

#[quick_bench(warmup_iters = 1, iters = 5)]
fn raw_load(b: bench::Bencher) {
    use crate::testing::calibration_image_paths;

    crate::testing::init_tracing();

    let paths = calibration_image_paths("Lights").or_else(|| calibration_image_paths("Flats"));
    let Some(paths) = paths else {
        eprintln!("No calibration images found, skipping");
        return;
    };
    let path = paths[0].clone();
    println!("Benchmarking load_raw on: {}", path.display());

    b.bench(|| load_raw(&path).unwrap());
}

/// Benchmark libraw's built-in demosaic at different quality levels.
/// For X-Trans: qual <= 2 -> Markesteijn 1-pass, qual >= 3 -> Markesteijn 3-pass.
/// For Bayer: 0=linear, 1=VNG, 2=PPG, 3=AHD, 11=DHT, 12=AAHD.
#[test]
#[ignore]
fn bench_load_raw_libraw_demosaic() {
    use crate::testing::calibration_image_paths;

    crate::testing::init_tracing();

    let paths = calibration_image_paths("Lights")
        .or_else(|| calibration_image_paths("Flats"))
        .expect("No calibration images found");
    let path = &paths[0];
    println!("Benchmarking libraw demosaic on: {}\n", path.display());

    let qualities = [
        (0, "linear"),
        (1, "VNG / Markesteijn 1-pass"),
        (2, "PPG / Markesteijn 1-pass"),
        (3, "AHD / Markesteijn 3-pass"),
        (11, "DHT / Markesteijn 3-pass"),
    ];

    for (qual, label) in &qualities {
        println!("--- user_qual={} ({}) ---", qual, label);

        // Warmup
        let _ = load_raw_libraw_demosaic(path, *qual).unwrap();

        let iterations = 3;
        let mut times = Vec::with_capacity(iterations);

        for i in 0..iterations {
            let start = Instant::now();
            let image = load_raw_libraw_demosaic(path, *qual).unwrap();
            let elapsed = start.elapsed();
            times.push(elapsed);
            println!(
                "  Run {}: {:.1}ms  ({}x{}x{})",
                i + 1,
                elapsed.as_secs_f64() * 1000.0,
                image.dimensions().width,
                image.dimensions().height,
                image.dimensions().channels,
            );
        }

        let avg_ms =
            times.iter().map(|t| t.as_secs_f64()).sum::<f64>() / iterations as f64 * 1000.0;
        let min_ms = times
            .iter()
            .map(|t| t.as_secs_f64())
            .fold(f64::MAX, f64::min)
            * 1000.0;
        println!("  Average: {:.1}ms, Best: {:.1}ms\n", avg_ms, min_ms);
    }
}

/// Compare our Markesteijn demosaic quality against libraw's reference implementation.
///
/// Since our pipeline doesn't apply white balance but libraw does, we use linear
/// regression per channel to remove scale/offset differences before comparison.
/// This isolates pure demosaic quality from color pipeline differences.
#[test]
#[ignore]
fn bench_markesteijn_quality_vs_libraw() {
    use crate::testing::calibration_image_paths;

    crate::testing::init_tracing();

    let paths = calibration_image_paths("Lights")
        .or_else(|| calibration_image_paths("Flats"))
        .expect("No calibration images found");
    let path = &paths[0];
    println!("Quality comparison on: {}\n", path.display());

    // Load with our Markesteijn implementation
    let start = Instant::now();
    let ours = load_raw(path).unwrap();
    let our_time = start.elapsed();
    println!(
        "Our Markesteijn:   {:.1}ms  ({}x{}x{})",
        our_time.as_secs_f64() * 1000.0,
        ours.dimensions().width,
        ours.dimensions().height,
        ours.dimensions().channels,
    );

    // Load with libraw's Markesteijn 1-pass (user_qual=1)
    let start = Instant::now();
    let reference = load_raw_libraw_demosaic(path, 1).unwrap();
    let ref_time = start.elapsed();
    println!(
        "libraw Markesteijn 1-pass: {:.1}ms  ({}x{}x{})",
        ref_time.as_secs_f64() * 1000.0,
        reference.dimensions().width,
        reference.dimensions().height,
        reference.dimensions().channels,
    );

    assert_eq!(ours.dimensions().width, reference.dimensions().width);
    assert_eq!(ours.dimensions().height, reference.dimensions().height);
    assert_eq!(ours.dimensions().channels, 3);
    assert_eq!(reference.dimensions().channels, 3);

    let width = ours.dimensions().width;
    let height = ours.dimensions().height;
    let border = 6;

    // Per-channel comparison using linear regression to remove WB/scale differences
    let channel_names = ["Red", "Green", "Blue"];
    let mut overall_mae = 0.0f64;

    println!("\n--- Ours vs libraw 1-pass (linear regression normalized) ---");
    for (c, name) in channel_names.iter().enumerate() {
        let stats = compare_channels(ours.channel(c), reference.channel(c), width, height, border);
        println!(
            "  {}: MAE={:.6}, PSNR={:.1}dB, r={:.6}  (scale={:.4}, offset={:.6})",
            name, stats.mae, stats.psnr, stats.correlation, stats.scale, stats.offset,
        );
        overall_mae += stats.mae;
    }

    println!(
        "\n  Avg MAE: {:.6}, Speedup: {:.1}x",
        overall_mae / 3.0,
        ref_time.as_secs_f64() / our_time.as_secs_f64()
    );

    // Compare against libraw 3-pass for context
    let start = Instant::now();
    let ref3 = load_raw_libraw_demosaic(path, 3).unwrap();
    let ref3_time = start.elapsed();
    println!(
        "\nlibraw Markesteijn 3-pass: {:.1}ms",
        ref3_time.as_secs_f64() * 1000.0
    );

    // Ours vs 3-pass
    let mut mae_3 = 0.0f64;
    for c in 0..3 {
        let stats = compare_channels(ours.channel(c), ref3.channel(c), width, height, border);
        mae_3 += stats.mae;
    }
    println!("  Ours vs 3-pass:   avg MAE={:.6}", mae_3 / 3.0);

    // 1-pass vs 3-pass (baseline: how much do libraw's own passes differ?)
    let mut mae_1v3 = 0.0f64;
    for c in 0..3 {
        let stats = compare_channels(reference.channel(c), ref3.channel(c), width, height, border);
        mae_1v3 += stats.mae;
    }
    println!(
        "  1-pass vs 3-pass: avg MAE={:.6}  (baseline)\n",
        mae_1v3 / 3.0
    );
}

/// Benchmark our RCD Bayer demosaic against libraw's built-in algorithms.
///
/// Compares: RCD (ours) vs PPG, AHD, DHT (libraw).
/// Requires a Bayer raw file in test_data/raw_samples/.
#[test]
#[ignore]
fn bench_bayer_rcd_demosaic() {
    crate::testing::init_tracing();

    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/raw_samples");
    let test_files = ["sample_canon.cr2", "raw-12bit-GBRG.dng", "sample.dng"];

    let path = test_files
        .iter()
        .map(|f| base.join(f))
        .find(|p| p.exists())
        .expect("No Bayer test file found in test_data/raw_samples/");
    let path = path.as_path();

    println!("Benchmarking Bayer demosaic on: {}\n", path.display());

    // Warmup
    let _ = load_raw(path).unwrap();

    // Our RCD demosaic
    println!("--- Our RCD demosaic ---");
    let iterations = 5;
    let mut times = Vec::with_capacity(iterations);
    let mut image = None;
    for i in 0..iterations {
        let start = Instant::now();
        let img = load_raw(path).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed);
        println!(
            "  Run {}: {:.1}ms  ({}x{}x{})",
            i + 1,
            elapsed.as_secs_f64() * 1000.0,
            img.dimensions().width,
            img.dimensions().height,
            img.dimensions().channels,
        );
        image = Some(img);
    }
    let rcd_avg = times.iter().map(|t| t.as_secs_f64()).sum::<f64>() / iterations as f64 * 1000.0;
    let rcd_best = times
        .iter()
        .map(|t| t.as_secs_f64())
        .fold(f64::MAX, f64::min)
        * 1000.0;
    println!("  Average: {:.1}ms, Best: {:.1}ms\n", rcd_avg, rcd_best);

    let _img = image.unwrap();

    // Compare with libraw algorithms
    for (qual, label) in [(2, "PPG"), (3, "AHD"), (11, "DHT")] {
        println!("--- libraw {label} (qual={qual}) ---");
        // Warmup
        let _ = load_raw_libraw_demosaic(path, qual).unwrap();

        let mut times = Vec::with_capacity(3);
        for i in 0..3 {
            let start = Instant::now();
            let img = load_raw_libraw_demosaic(path, qual).unwrap();
            let elapsed = start.elapsed();
            times.push(elapsed);
            println!(
                "  Run {}: {:.1}ms  ({}x{}x{})",
                i + 1,
                elapsed.as_secs_f64() * 1000.0,
                img.dimensions().width,
                img.dimensions().height,
                img.dimensions().channels,
            );
        }
        let avg = times.iter().map(|t| t.as_secs_f64()).sum::<f64>() / 3.0 * 1000.0;
        let best = times
            .iter()
            .map(|t| t.as_secs_f64())
            .fold(f64::MAX, f64::min)
            * 1000.0;
        println!(
            "  Average: {:.1}ms, Best: {:.1}ms  (RCD is {:.1}x)\n",
            avg,
            best,
            best / rcd_best
        );
    }
}

/// Compare our RCD Bayer demosaic quality against libraw's AHD reference.
///
/// Uses linear regression per channel to remove WB/scale differences,
/// then computes MAE, PSNR, and Pearson correlation.
#[test]
#[ignore]
fn bench_bayer_rcd_quality_vs_libraw() {
    crate::testing::init_tracing();

    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/raw_samples");
    let test_files = ["sample_canon.cr2", "raw-12bit-GBRG.dng", "sample.dng"];

    let path = test_files
        .iter()
        .map(|f| base.join(f))
        .find(|p| p.exists())
        .expect("No Bayer test file found in test_data/raw_samples/");
    let path = path.as_path();

    println!("Bayer quality comparison on: {}\n", path.display());

    // Our RCD
    let start = Instant::now();
    let ours = load_raw(path).unwrap();
    let our_time = start.elapsed();
    println!(
        "Our RCD:       {:.1}ms  ({}x{}x{})",
        our_time.as_secs_f64() * 1000.0,
        ours.dimensions().width,
        ours.dimensions().height,
        ours.dimensions().channels,
    );

    let width = ours.dimensions().width;
    let height = ours.dimensions().height;
    let border = 6;
    let channel_names = ["Red", "Green", "Blue"];

    // Compare against libraw AHD (qual=3), PPG (qual=2), DHT (qual=11)
    for (qual, label) in [(3, "AHD"), (2, "PPG"), (11, "DHT")] {
        let start = Instant::now();
        let reference = load_raw_libraw_demosaic(path, qual).unwrap();
        let ref_time = start.elapsed();
        println!(
            "libraw {label}:     {:.1}ms",
            ref_time.as_secs_f64() * 1000.0
        );

        if ours.dimensions().width != reference.dimensions().width
            || ours.dimensions().height != reference.dimensions().height
        {
            println!("  Dimension mismatch, skipping comparison\n");
            continue;
        }

        println!("  --- Ours vs libraw {label} (linear regression normalized) ---");
        let mut overall_mae = 0.0f64;
        for (c, name) in channel_names.iter().enumerate() {
            let stats =
                compare_channels(ours.channel(c), reference.channel(c), width, height, border);
            println!(
                "    {}: MAE={:.6}, PSNR={:.1}dB, r={:.6}  (scale={:.4}, offset={:.6})",
                name, stats.mae, stats.psnr, stats.correlation, stats.scale, stats.offset,
            );
            overall_mae += stats.mae;
        }
        println!(
            "    Avg MAE: {:.6}, Speedup: {:.1}x\n",
            overall_mae / 3.0,
            ref_time.as_secs_f64() / our_time.as_secs_f64()
        );
    }
}

/// Benchmark just the RCD demosaic core (excluding raw file loading/normalization).
///
/// Uses a synthetic Bayer image to isolate demosaic performance.
#[test]
#[ignore]
fn bench_rcd_demosaic_core() {
    use demosaic::bayer::CfaPattern;

    println!("Benchmarking RCD demosaic core (synthetic data)\n");

    for (w, h) in [(1000, 1000), (4000, 3000), (6000, 4000)] {
        let mut data = vec![0.0f32; w * h];
        // Fill with a gradient pattern for realistic workload
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = ((x + y) as f32 / (w + h) as f32).min(1.0);
            }
        }

        let bayer = BayerImage::with_margins(&data, w, h, w, h, 0, 0, CfaPattern::Rggb);

        // Warmup
        let _ = demosaic_bayer(&bayer);

        let iterations = 5;
        let mut times = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            let _rgb = demosaic_bayer(&bayer);
            let elapsed = start.elapsed();
            times.push(elapsed);
        }

        let avg_ms =
            times.iter().map(|t| t.as_secs_f64()).sum::<f64>() / iterations as f64 * 1000.0;
        let best_ms = times
            .iter()
            .map(|t| t.as_secs_f64())
            .fold(f64::MAX, f64::min)
            * 1000.0;
        let mpix = (w * h) as f64 / 1e6;
        println!(
            "  {w}x{h} ({mpix:.1} MP): avg {avg_ms:.1}ms, best {best_ms:.1}ms ({:.1} MP/s)",
            mpix / (best_ms / 1000.0)
        );
    }
    println!();
}

/// Load raw file using libraw's built-in demosaic (for benchmarking comparison).
fn load_raw_libraw_demosaic(path: &Path, user_qual: i32) -> Result<AstroImage> {
    let raw = open_raw(path)?;

    // Set demosaic quality before processing
    unsafe {
        (*raw.inner).params.user_qual = user_qual;
    }

    let (pixels, out_width, out_height, num_channels) = raw.demosaic_libraw_fallback()?;

    let dimensions = ImageDimensions::new(out_width, out_height, num_channels);
    Ok(AstroImage::from_pixels(dimensions, pixels))
}

#[derive(Debug)]
struct ChannelCompareStats {
    mae: f64,
    psnr: f64,
    correlation: f64,
    scale: f64,
    offset: f64,
}

/// Compare two channels using linear regression to remove scale/offset differences.
fn compare_channels(
    a: &crate::common::Buffer2<f32>,
    b: &crate::common::Buffer2<f32>,
    width: usize,
    height: usize,
    border: usize,
) -> ChannelCompareStats {
    // Linear regression: b ~ scale * a + offset
    let mut sum_a = 0.0f64;
    let mut sum_b = 0.0f64;
    let mut sum_a2 = 0.0f64;
    let mut sum_ab = 0.0f64;
    let mut sum_b2 = 0.0f64;
    let mut n = 0u64;

    for y in border..(height - border) {
        for x in border..(width - border) {
            let idx = y * width + x;
            let av = a[idx] as f64;
            let bv = b[idx] as f64;
            sum_a += av;
            sum_b += bv;
            sum_a2 += av * av;
            sum_ab += av * bv;
            sum_b2 += bv * bv;
            n += 1;
        }
    }

    let nf = n as f64;
    let denom = nf * sum_a2 - sum_a * sum_a;
    let (scale, offset) = if denom.abs() > 1e-30 {
        let s = (nf * sum_ab - sum_a * sum_b) / denom;
        let o = (sum_b - s * sum_a) / nf;
        (s, o)
    } else {
        (1.0, 0.0)
    };

    // Compute residuals after regression
    let mut sum_abs_err = 0.0f64;
    let mut sum_sq_err = 0.0f64;
    let mean_b = sum_b / nf;

    for y in border..(height - border) {
        for x in border..(width - border) {
            let idx = y * width + x;
            let predicted = (a[idx] as f64) * scale + offset;
            let actual = b[idx] as f64;
            let diff = predicted - actual;
            sum_abs_err += diff.abs();
            sum_sq_err += diff * diff;
        }
    }

    let mae = sum_abs_err / nf;
    let mse = sum_sq_err / nf;
    let psnr = if mse > 0.0 {
        10.0 * (mean_b * mean_b / mse).log10()
    } else {
        f64::INFINITY
    };

    // Pearson correlation
    let correlation = (nf * sum_ab - sum_a * sum_b)
        / ((nf * sum_a2 - sum_a * sum_a).sqrt() * (nf * sum_b2 - sum_b * sum_b).sqrt());

    ChannelCompareStats {
        mae,
        psnr,
        correlation,
        scale,
        offset,
    }
}
