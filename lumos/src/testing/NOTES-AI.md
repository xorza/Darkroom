# Testing Module

Crate-wide testing utilities for lumos. Exposed as `pub mod testing` gated behind `#[cfg(test)]` in `lib.rs`.

## Module Structure

```
testing/
  mod.rs              - TestRng, background estimation, calibration helpers, tracing init
  real_data/
    mod.rs             - Module declaration
    pipeline_bench.rs  - Full pipeline benchmark (CFA masters -> calibrate -> register -> stack)
  synthetic/
    mod.rs             - Re-exports, presets (sparse/dense/crowded/faint/elliptical), StarFieldBuilder
    star_field.rs      - StarFieldConfig, generate_star_field(), GroundTruthStar, preset configs
    star_profiles.rs   - PSF renderers: Gaussian, Moffat, elliptical, saturated, cosmic ray
    backgrounds.rs     - Background adders: uniform, gradient, vignette, amp glow, nebula
    background_map.rs  - BackgroundEstimate factories: uniform, gradient, vignette
    patterns.rs        - Test patterns: uniform, gradients, checkerboard, noise
    stamps.rs          - Small star stamps for centroid/fitting: gaussian, moffat, elliptical, multi-spot
    artifacts.rs       - Cosmic rays, Bayer pattern artifacts
    transforms.rs      - Positioned trait, geometric transforms for registration testing
```

Related test utilities outside this module:
- `star_detection/centroid/test_utils.rs` - `add_noise` (Box-Muller), `approx_eq`, `compute_hessian_gradient`
- `star_detection/tests/common/output/` - Image writers, comparison visualization, DetectionMetrics

## TestRng

Deterministic LCG for reproducible test data. Knuth MMIX multiplier (6364136223846793005) with increment 1.

```rust
pub struct TestRng { state: u64 }
// next_u64: state = state * 6364136223846793005 + 1; return state
// next_f32: (next_u64() >> 33) as f32 / 2^31
// next_f64: (next_u64() >> 11) as f64 / 2^53
// next_gaussian_f32: Box-Muller transform returning f32
```

**Properties:**
- Full 2^64 period (odd increment guarantees this for power-of-2 modulus LCG)
- Deterministic given seed -- same seed always produces same sequence
- No external dependency (no `rand` crate needed)
- Used by ~21 call sites across 11 files
- `next_f64` uses 53-bit precision (full f64 mantissa coverage)
- `next_f32` uses 31 bits (sufficient for f32's 23-bit mantissa)
- `next_gaussian_f32()` provides Box-Muller Gaussian sampling (mean=0, stddev=1)

**Known limitations:**
- LCGs have known sequential correlation in low-order bits, but the right-shift by 33/11 uses only the high bits, which mitigates this.
- Increment 1 instead of Knuth's recommended 1442695040888963407 -- still gives full period but may have slightly worse spectral properties. For test data generation this is not a concern.

## Positioned Trait

```rust
pub trait Positioned: Clone {
    fn pos(&self) -> DVec2;
    fn with_pos(&self, pos: DVec2) -> Self;
}
```

Implemented for `DVec2` and `Star`. Enables generic transform functions (`translate_impl`, `transform_impl`, `add_noise_impl`, `remove_random_impl`, `filter_to_bounds_impl`, `translate_with_overlap_impl`) that work on either bare positions or full Star structs.

**Design:** Clean separation with private generic `_impl` functions and public typed wrappers (e.g., `translate_stars` for DVec2, `translate_star_list` for Star). Verified consistent behavior via `test_positioned_trait_dvec2_and_star_consistent`.

## Synthetic Data Generation

### StarFieldConfig + generate_star_field()
Comprehensive star field generator with:
- Crowding types: Uniform, Clustered (70/30 core/halo), Gradient
- Elongation types: None, Uniform, Varying, FieldRotation
- PSF profiles: Gaussian or Moffat
- Saturation simulation
- Backgrounds: uniform, gradient, vignette, nebula
- Cosmic rays, Bayer artifacts, Gaussian noise
- Returns `(Buffer2<f32>, Vec<GroundTruthStar>)` for ground-truth validation

### StarFieldBuilder
Fluent builder API wrapping StarFieldConfig. All builder methods documented. Convenience presets: `sparse_field()`, `dense_field()`, `crowded_cluster()`, `faint_field()`, `elliptical_field()`.

### Star Profiles (star_profiles.rs)
- `render_gaussian_star` - 4-sigma radius, additive
- `render_moffat_star` - 8-alpha radius, additive
- `render_elliptical_star` - Rotated coordinate system
- `render_saturated_star` - Clipped at saturation_level
- `render_cosmic_ray` / `render_cosmic_ray_extended` - Point + neighbor bleed
- `fwhm_to_sigma`, `sigma_to_fwhm`, `fwhm_to_moffat_alpha`, `moffat_fwhm`

### Stamps (stamps.rs)
Small image patches for centroid/fitting tests:
- `gaussian()`, `moffat()`, `elliptical()` - Single star with configurable params
- `star_field()` - Multiple random stars
- `gaussian_spot()`, `multi_gaussian_spots()` - For phase correlation tests
- `benchmark_star_field()` - Large field for detection benchmarks

### Patterns (patterns.rs)
Basic test patterns for benchmarks and interpolation tests:
- `uniform()`, `horizontal_gradient()`, `vertical_gradient()`, `diagonal_gradient()`, `radial_gradient()`
- `checkerboard()`, `checkerboard_offset()`
- `add_noise()`, `noise()` - Deterministic uniform noise

### Transforms (transforms.rs)
For registration testing:
- Position generation: `generate_random_positions`, `generate_random_positions_with_margin`
- Transforms: translate, rotate, scale, general similarity
- Perturbations: add noise, remove random (missed detections), add spurious (false positives)
- Filtering: bounds filtering, translate with overlap

## Calibration and Real-Data Helpers

- `estimate_background()` - Convenience wrapper for `stages::background::estimate_background`
- `calibration_dir()` / `calibration_masters_dir()` - Read `LUMOS_CALIBRATION_DIR` env var
- `load_calibration_images()`, `first_raw_file()`, `calibration_image_paths()` - Load real data
- `init_tracing()` - One-shot tracing subscriber with RUST_LOG support

## Implementation Review

### Strengths

1. **Centralized determinism.** All synthetic data uses TestRng with seed propagation. Reproducibility is tested explicitly (e.g., `test_reproducibility`, `test_builder_reproducibility`, `test_noise_reproducibility`).

2. **Comprehensive synthetic data coverage.** The star field generator covers virtually all conditions an astronomical star detector would encounter: crowding, elongation, saturation, backgrounds, artifacts. The `GroundTruthStar` struct enables precise validation.

3. **Clean trait abstraction.** The Positioned trait eliminates code duplication for geometric transforms across DVec2 and Star types. The public API is clean with consistent naming (`_stars` for DVec2, `_star_list` for Star).

4. **Builder pattern.** `StarFieldBuilder` provides a discoverable, ergonomic API for custom field generation alongside the preset functions.

5. **Layered architecture.** Background, profile, noise, and artifact generation are independent composable stages, allowing tests to use exactly the complexity they need. This matches the architecture of industry-standard tools like photutils (which has separate `make_model_params`, `make_model_image`, and `make_noise_image` stages) and GalSim (which uses composable PSF + pixel + noise pipelines).

6. **Good test coverage within the module.** Each submodule has its own `#[cfg(test)] mod tests` validating basic correctness.

7. **No external RNG dependency.** TestRng avoids pulling in the `rand` crate for test-only use. This is a valid design choice -- the Rust `rand` crate explicitly warns that `SmallRng` and `StdRng` are not reproducible across versions or platforms, so using them for deterministic test data is unsuitable unless pinning a named algorithm crate (e.g., `rand_xoshiro`, `rand_chacha`).

8. **High-bit extraction in TestRng.** The right-shift by 33 bits (for f32) and 11 bits (for f64) correctly mitigates the well-known LCG low-bit correlation problem, where the low b bits of a power-of-2 modulus LCG repeat with period 2^(b-2).

### Issues

1. **PSF rendering uses point-sampling, not pixel integration.** All star profile renderers (`render_gaussian_star`, `render_moffat_star`, `render_elliptical_star`) evaluate the PSF at pixel centers only. Scientific image simulators (GalSim, WebbPSF, photutils) integrate the PSF over each pixel area, either analytically or via subpixel oversampling. Point-sampling introduces systematic centroid bias (up to 0.06 pixels for Gaussian PSFs) because it does not account for the fact that a CCD pixel integrates photons across its entire area, not just at its center. This matters for centroid/fitting test accuracy. For most detection and registration tests (which only need approximate positions) the current approach is adequate, but for photometry or sub-pixel centroid validation, this is a known source of systematic error.

2. **Box-Muller wastes one Gaussian sample per call.** `next_gaussian_f32()` generates two uniform samples to produce one Gaussian value via Box-Muller. The standard Box-Muller transform produces two independent Gaussian samples per pair of uniform inputs; the second is discarded. NumPy's implementation caches the second value for the next call. For test data generation the performance impact is negligible, but it is worth noting.

3. **BackgroundEstimate helpers live outside the testing module.** `background_map.rs` produces `BackgroundEstimate` structs for test use, while `star_detection/tests/common/output/metrics.rs` contains `DetectionMetrics` and pass/fail criteria. Both are test infrastructure but live in different modules. The `DetectionMetrics` infrastructure could be promoted to `testing/` for broader reuse.

4. **No Poisson noise generator.** Astronomical photon noise follows Poisson statistics, not Gaussian. The standard CCD noise model is: `signal = Poisson(photon_flux * gain) / gain + Poisson(dark_current * exposure) / gain + N(0, read_noise)`. For realistic synthetic images (especially at low flux where Poisson deviates from Gaussian), a Poisson noise generator would be more physically accurate. The Astropy CCD Reduction Guide uses separate Poisson draws for sky, dark current, and photon signal. Current Gaussian noise is adequate for most detection/registration tests but insufficient for photometry validation.

5. **patterns::add_noise uses uniform noise, star_field uses Gaussian noise.** The naming is the same (`add_noise`) but the distributions differ. `patterns::add_noise` generates uniform noise `(hash - 0.5) * 2 * amplitude`, while `star_field::add_gaussian_noise` generates proper Gaussian via Box-Muller. This can confuse test authors.

6. **benchmark_star_field uses a third noise method (raw hash).** `stamps::benchmark_star_field` generates noise via `((i as u64).wrapping_mul(2654435761).wrapping_add(seed)) as f32 / (u64::MAX as f32)`, a raw multiplicative hash. This is a different approach from both TestRng-based uniform (`patterns::add_noise`) and TestRng-based Gaussian (`star_field::add_gaussian_noise`). Three different noise generation methods across the module is confusing.

7. **No test_data module for fixture files.** The `lumos/test_data/` directory exists (untracked per git status) but there is no test utility for locating or loading fixture files from the test data directory in a portable way.

8. **`generate_globular_cluster` does not return ground truth.** Unlike `generate_star_field` which returns `(Buffer2<f32>, Vec<GroundTruthStar>)`, `generate_globular_cluster` returns only `Buffer2<f32>`. Stars that fall outside the image are silently skipped, making the actual star count unknown. This limits its usefulness for quantitative validation (completeness/false-positive metrics).

### Comparison with Industry Standards

**Reference tools surveyed:** photutils (astropy), GalSim, WebbPSF, Astropy CCD Reduction Guide.

| Feature | This Module | photutils | GalSim |
|---|---|---|---|
| Gaussian PSF | Yes (point-sample) | Yes (pixel-integrated PRF) | Yes (Fourier or photon-shooting) |
| Moffat PSF | Yes | Yes | Yes |
| Elliptical PSF | Yes | Yes | Yes |
| Pixel integration | No | Yes | Yes |
| Poisson noise | No | Yes | Yes |
| Gaussian noise | Yes | Yes | Yes |
| Read noise model | No | No (external) | Yes |
| Dark current model | No | No (external) | Yes |
| Ground truth output | Yes | Yes (input table) | Yes (input catalog) |
| Magnitude-to-flux | Yes (Pogson) | Yes | Yes |
| Composable pipeline | Yes | Yes | Yes |
| Deterministic seed | Yes | Yes (numpy seed) | Yes |
| Cosmic rays | Yes | No | No (external) |
| Bayer artifacts | Yes | No | No |
| Background models | Yes (4 types) | Manual | Manual |

The module has broader artifact/background coverage than photutils but lacks the physics-based noise models and pixel-integrated PSF rendering that both photutils and GalSim provide.

## Recommendations

1. **Rename noise functions for clarity.** Rename `patterns::add_noise` to `add_uniform_noise` and document the distribution in each function's doc comment. Consider consolidating the three noise generation methods (TestRng uniform, TestRng Gaussian, raw hash) into a consistent API.

2. **Add a `next_poisson()` method** (or standalone function) for photon noise simulation. A simple Knuth algorithm suffices for lambda < 30; for larger lambda, use the Gaussian approximation N(lambda, sqrt(lambda)). This would enable realistic CCD noise models: `Poisson(signal) + Poisson(dark) + N(0, read_noise)`.

3. **Consider pixel-integrated PSF rendering** for centroid/fitting validation tests. Either use analytical integration for Gaussian PSFs (via the error function: `erf((x+0.5-cx) / (sigma*sqrt(2))) - erf((x-0.5-cx) / (sigma*sqrt(2)))`) or add a 3x3 or 5x5 subpixel oversampling option. This removes the systematic centroid bias from point-sampling and is important for tests validating sub-pixel centroid accuracy.

4. **Consolidate DetectionMetrics into testing/.** Move `star_detection/tests/common/output/metrics.rs` (and potentially comparison.rs) into `testing/` so registration tests and future modules can compute detection metrics against ground truth without depending on `star_detection::tests`.

5. **Add a test_data path helper.** Similar to `calibration_dir()`, add a function that returns `PathBuf` to `lumos/test_data/` for loading small fixture images in tests that need them.

6. **Add ground truth return to `generate_globular_cluster`.** Change the return type to `(Buffer2<f32>, Vec<GroundTruthStar>)` matching the `generate_star_field` API, so it can be used for quantitative detection validation in crowded fields.
