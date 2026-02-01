# Moffat PSF Fitting Module

2D Moffat profile fitting for high-precision stellar centroid computation using Levenberg-Marquardt optimization with SIMD acceleration.

## Overview

The Moffat profile is a better model for stellar PSFs than Gaussian because it has extended wings that match atmospheric seeing:

```
f(x,y) = A × (1 + ((x-x₀)² + (y-y₀)²) / α²)^(-β) + B
```

where:
- `α` (alpha) is the core width parameter
- `β` (beta) controls the wing slope (typically 2.5-4.5)
- `A` is the amplitude
- `B` is the background level

This achieves ~0.01 pixel centroid accuracy while providing more accurate flux and FWHM estimates than Gaussian fitting.

## Module Structure

```
moffat_fit/
├── mod.rs          # Main fitting functions and L-M optimizer
├── simd/
│   ├── mod.rs      # SIMD dispatch and feature detection
│   ├── avx2.rs     # AVX2+FMA implementation (8 pixels parallel)
│   ├── sse.rs      # SSE4.1 implementation (4 pixels parallel)
│   ├── neon.rs     # NEON implementation for aarch64
│   └── tests.rs    # SIMD-specific tests
├── tests.rs        # Integration tests
└── bench.rs        # Performance benchmarks
```

## Performance

Benchmark results on 6K image with 10,000 stars:

| Optimization | Time | Improvement |
|-------------|------|-------------|
| Baseline | 728ms | - |
| Cached power computations | 620ms | -15% |
| SIMD vectorization | 155ms | -73% |
| **Total** | **155ms** | **-78.7%** |

### SIMD Speedups (Jacobian computation)

| Size | Pixels | Scalar | SSE4 | AVX2 | Speedup |
|------|--------|--------|------|------|---------|
| Small | 289 (17×17) | 2.36µs | 1.64µs | 1.52µs | 1.55× |
| Medium | 625 (25×25) | 10.5µs | 6.83µs | 6.94µs | 1.52× |
| Large | 1089 (33×33) | 18.2µs | 12.1µs | 12.1µs | 1.50× |

## Implementation Details

### Fitting Modes

1. **Fixed Beta (5 parameters)**: `[x₀, y₀, amplitude, alpha, background]`
   - Uses SIMD-optimized L-M optimizer
   - Best for batch processing with known β

2. **Variable Beta (6 parameters)**: `[x₀, y₀, amplitude, alpha, beta, background]`
   - Uses generic L-M optimizer
   - Best when β varies across the field

### SIMD Architecture

The SIMD implementation uses a **hybrid approach**:
- **Scalar `powf()`** for the expensive power operation (accuracy required for L-M convergence)
- **SIMD for all other arithmetic** (dx, dy, r², divisions, multiplications)

This provides most of the SIMD benefit while maintaining numerical accuracy for gradient computation.

Runtime dispatch automatically selects the best available implementation:
1. AVX2+FMA on x86_64 (8 pixels parallel)
2. SSE4.1 on x86_64 (4 pixels parallel)
3. NEON on aarch64 (4 pixels parallel)
4. Scalar fallback on other platforms

### Key Optimizations

1. **Cached Power Computations**: Single `powf()` call per pixel
   ```rust
   // Compute u^(-beta) once, derive u^(-beta-1) from it
   let u_neg_beta = u.powf(-beta);
   let u_neg_beta_m1 = u_neg_beta / u;  // u^(-beta-1)
   ```

2. **Chi² Stagnation Detection**: Handles numerical precision limits
   ```rust
   let chi2_rel_change = (prev_chi2 - new_chi2) / prev_chi2.max(1e-30);
   if chi2_rel_change < 1e-6 {
       converged = true;
   }
   ```

3. **Buffer Reuse**: L-M optimizer reuses Jacobian/residual buffers across iterations

4. **Stack Allocation**: Uses ArrayVec for stamp extraction (no heap allocation)

### Why Not Full SIMD powf?

The theoretical 4× (SSE) or 8× (AVX2) speedup isn't achieved because:
- `u.powf(-beta)` must be computed per-pixel for accuracy
- Fast vectorized pow approximations (exp(y*log(x))) have ~10⁻⁵ relative error
- L-M convergence depends on accurate gradients

Libraries like [SLEEF](https://sleef.org/) provide vectorized `powf` with 1 ULP accuracy, but would require nightly Rust features or additional dependencies.

## Usage

```rust
use lumos::star_detection::centroid::moffat_fit::{
    fit_moffat_2d, fit_moffat_2d_weighted, MoffatFitConfig
};

// Basic fitting
let config = MoffatFitConfig::default();
let result = fit_moffat_2d(&pixels, cx, cy, stamp_radius, background, &config)?;

// Weighted fitting (optimal for known noise characteristics)
let result = fit_moffat_2d_weighted(
    &pixels, cx, cy, stamp_radius,
    background, noise, gain, read_noise,
    &config
)?;

println!("Center: ({:.3}, {:.3})", result.x, result.y);
println!("FWHM: {:.2} pixels", result.fwhm);
println!("Converged: {} in {} iterations", result.converged, result.iterations);
```

## Theory

### Moffat Profile Properties

From [Trujillo et al. (2001)](https://academic.oup.com/mnras/article/328/3/977/1247204):
- Best fit to atmospheric turbulence theory when **β ≈ 4.765**
- For seeing-limited observations, **β ≈ 2.5** is typical
- Contains Gaussian PSF as limiting case (β → ∞)
- β must be > 1 to ensure finite integral

### FWHM Relationship

```
FWHM = 2α × √(2^(1/β) - 1)
```

### Numerical Stability

From [Astropy](https://docs.astropy.org/en/stable/api/astropy.modeling.functional_models.Moffat2D.html) and [Photutils](https://photutils.readthedocs.io/en/latest/api/photutils.psf.MoffatPSF.html):
- Moffat functions are numerically better behaved than Gaussians for narrow PSFs
- Avoid exponential overflow issues that occur with Gaussians when σ < 1 pixel

## Limitations

1. **Circular PSF only**: No support for elliptical Moffat (rotation, axis ratio)
2. **Variable beta not SIMD**: Only fixed-beta mode uses SIMD optimization
3. **Single-threaded**: No parallel processing of multiple stars (done at detector level)

## Running Benchmarks

```bash
# Run all moffat_fit benchmarks
cargo test -p lumos --release 'moffat_fit::bench' -- --ignored --nocapture

# Run SIMD-specific benchmarks
cargo test -p lumos --release 'bench_jacobian\|bench_chi2\|bench_batch' -- --ignored --nocapture
```

## References

1. [Trujillo et al. 2001](https://academic.oup.com/mnras/article/328/3/977/1247204) - Moffat PSF theory
2. [Astropy Moffat2D](https://docs.astropy.org/en/stable/api/astropy.modeling.functional_models.Moffat2D.html) - Reference implementation
3. [Photutils MoffatPSF](https://photutils.readthedocs.io/en/latest/api/photutils.psf.MoffatPSF.html) - Jacobian computation
4. [SLEEF Library](https://sleef.org/) - Vectorized math functions
5. [Fast Vectorizable Math Approximations](http://gallium.inria.fr/blog/fast-vectorizable-math-approx/) - SIMD optimization techniques
