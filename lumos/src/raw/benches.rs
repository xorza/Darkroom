use std::time::Instant;

use super::*;

#[test]
#[ignore]
fn quick_bench() {
    use crate::testing::calibration_image_paths;

    crate::testing::init_tracing();

    let Some(paths) = calibration_image_paths("Lights") else {
        eprintln!("No Lights found, trying Flats");
        let Some(paths) = calibration_image_paths("Flats") else {
            eprintln!("No calibration images found, skipping");
            return;
        };
        run_load_bench(&paths);
        return;
    };
    run_load_bench(&paths);
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

/// Load raw file using libraw's built-in demosaic (for benchmarking comparison).
fn load_raw_libraw_demosaic(path: &Path, user_qual: i32) -> Result<AstroImage> {
    let buf = fs::read(path)?;
    let inner = unsafe { sys::libraw_init(0) };
    assert!(!inner.is_null());
    let guard = LibrawGuard(inner);

    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    assert_eq!(ret, 0, "libraw open_buffer failed: {ret}");

    let ret = unsafe { sys::libraw_unpack(inner) };
    assert_eq!(ret, 0, "libraw unpack failed: {ret}");

    // Set demosaic quality before processing
    unsafe {
        (*inner).params.user_qual = user_qual;
    }

    let (pixels, out_width, out_height, num_channels) = process_unknown_libraw_fallback(inner)?;

    drop(guard);

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

fn run_load_bench(paths: &[std::path::PathBuf]) {
    let path = &paths[0];
    println!("Benchmarking load_raw on: {}", path.display());

    // Warmup
    let _ = load_raw(path).unwrap();

    let iterations = 5;
    let mut times = Vec::with_capacity(iterations);

    for i in 0..iterations {
        let start = Instant::now();
        let image = load_raw(path).unwrap();
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

    let avg_ms = times.iter().map(|t| t.as_secs_f64()).sum::<f64>() / iterations as f64 * 1000.0;
    let min_ms = times
        .iter()
        .map(|t| t.as_secs_f64())
        .fold(f64::MAX, f64::min)
        * 1000.0;
    println!("\n  Average: {:.1}ms", avg_ms);
    println!("  Best:    {:.1}ms", min_ms);
}
