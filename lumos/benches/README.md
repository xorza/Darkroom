# Lumos Benchmarks

Performance benchmarks for the Lumos astrophotography image processing library.

## Directory Structure

```
benches/
├── star_detection/        # Star detection pipeline stages
│   ├── convolution.rs     # Gaussian kernels, image convolution
│   ├── background.rs      # Background estimation
│   ├── threshold.rs       # Threshold mask creation
│   ├── deblend.rs         # Multi-threshold deblending
│   ├── centroid.rs        # Centroid refinement
│   └── cosmic_ray.rs      # Cosmic ray detection
│
├── image_processing/      # Image processing operations
│   ├── demosaic_bayer.rs  # Bayer pattern demosaicing
│   ├── demosaic_xtrans.rs # X-Trans pattern demosaicing
│   ├── hot_pixels.rs      # Hot pixel correction
│   └── median_filter.rs   # Median filtering
│
├── registration/          # Image registration
│   └── pipeline.rs        # Star matching, RANSAC, warping
│
├── stacking/              # Image stacking algorithms
│   ├── mean.rs            # Mean stacking
│   ├── median.rs          # Median stacking
│   └── sigma_clipped.rs   # Sigma-clipped mean stacking
│
├── math/                  # Core math operations
│   └── operations.rs      # Sum, accumulate, scale, statistics
│
├── pipeline/              # End-to-end benchmarks
│   └── full_pipeline.rs   # Complete processing pipeline
│
├── results/               # Saved benchmark results
└── run_benches.sh         # Benchmark runner script
```

## Running Benchmarks

### Prerequisites

Some benchmarks require test data. Set the environment variable:

```bash
export LUMOS_CALIBRATION_DIR=/path/to/calibration/data
```

The calibration directory should contain:
- `Lights/` - Light frames (RAW files)
- `calibration_masters/` - Master calibration frames

### Using the Runner Script

```bash
cd lumos

# Show available options
./benches/run_benches.sh --help

# Run benchmarks by category
./benches/run_benches.sh star_detection
./benches/run_benches.sh image_processing
./benches/run_benches.sh registration
./benches/run_benches.sh stacking
./benches/run_benches.sh math
./benches/run_benches.sh pipeline

# Run multiple categories
./benches/run_benches.sh star_detection stacking

# Run all benchmarks
./benches/run_benches.sh all

# Run a specific benchmark
./benches/run_benches.sh star_detection_convolution
```

### Using Cargo Directly

```bash
cd lumos

# Run a specific benchmark
cargo bench --features bench --bench star_detection_convolution

# Run all benchmarks (requires significant memory due to LTO)
cargo bench --features bench

# Run with custom Criterion options
cargo bench --features bench --bench star_detection_convolution -- --sample-size 50
```

## Benchmark Categories

| Category | Count | Description |
|----------|-------|-------------|
| `star_detection` | 6 | Star detection pipeline: convolution, background, threshold, deblend, centroid, cosmic ray |
| `image_processing` | 4 | Demosaicing (Bayer/X-Trans), hot pixels, median filter |
| `registration` | 1 | Full registration: star matching, RANSAC, image warping |
| `stacking` | 3 | Mean, median, and sigma-clipped mean stacking |
| `math` | 1 | Core operations: sum, accumulate, scale, squared differences |
| `pipeline` | 1 | End-to-end astrophotography processing |

## Implementation Variants

Benchmarks compare different implementations where available:

| Algorithm | Scalar | SIMD | GPU |
|-----------|--------|------|-----|
| Convolution | Yes | Yes | - |
| Background estimation | Yes | Yes | - |
| Threshold mask | Yes | Yes | - |
| Deblending | Yes | - | - |
| Centroid refinement | Yes | - | - |
| Cosmic ray detection | Yes | Yes | - |
| Bayer demosaic | Yes | - | - |
| X-Trans demosaic | Yes | Yes | - |
| Median filter | Yes | Yes | - |
| Hot pixel correction | Yes | - | - |
| Image warping | Yes | - | Yes |
| Mean stacking | Yes | Yes | - |
| Median stacking | Yes | - | - |
| Sigma-clipped stacking | Yes | - | - |
| Math operations | Yes | Yes | - |

## Adding New Benchmarks

1. Create a new `.rs` file in the appropriate category directory
2. Follow the standard benchmark template:

```rust
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithm_name");
    
    for size in [512, 1024, 2048, 4096] {
        let pixels = size * size;
        group.throughput(Throughput::Elements(pixels as u64));
        
        // Scalar implementation
        group.bench_function(
            BenchmarkId::new("scalar", format!("{}x{}", size, size)),
            |b| b.iter(|| scalar_implementation(...))
        );
        
        // SIMD implementation (if available)
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        group.bench_function(
            BenchmarkId::new("simd", format!("{}x{}", size, size)),
            |b| b.iter(|| simd_implementation(...))
        );
    }
    
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
```

3. Add the benchmark to `Cargo.toml`:

```toml
[[bench]]
name = "benchmark_name"
path = "benches/category/filename.rs"
harness = false
required-features = ["bench"]
```

4. Update `run_benches.sh` to include the new benchmark in its category array

## Benchmark Guidelines

- **Atomic tests**: Each benchmark should test ONE algorithm stage
- **Consistent data**: Use standard image sizes (512, 1024, 2048, 4096)
- **Report throughput**: Use `Throughput::Elements()` or `Throughput::Bytes()`
- **Reproducible**: Use deterministic test data generation (seeded RNG)
- **Descriptive IDs**: Use `BenchmarkId::new("variant", "parameters")`

## Interpreting Results

Criterion outputs results in `target/criterion/`. Key metrics:

- **Time**: Mean execution time per iteration
- **Throughput**: Elements or bytes processed per second
- **Change**: Performance difference from baseline (if saved)

Example output:
```
convolution/simd/2048x2048
                        time:   [1.2345 ms 1.2456 ms 1.2567 ms]
                        thrpt:  [3.3456 Gelem/s 3.3789 Gelem/s 3.4123 Gelem/s]
                 change: [-5.1234% -4.5678% -4.0123%] (p = 0.00 < 0.05)
                 Performance has improved.
```

## Saved Results

Previous benchmark results are stored in `results/` for reference. These can be used to track performance over time or compare implementations.
