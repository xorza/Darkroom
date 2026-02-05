# MAGSAC++ Integration Design Document

## Overview

This document describes how to integrate MAGSAC++ (Marginalizing Sample Consensus) threshold-free scoring into the existing RANSAC implementation. MAGSAC++ eliminates the need for manual `inlier_threshold` tuning by marginalizing over a range of noise scales.

## Problem Statement

The current RANSAC implementation uses MSAC scoring with a fixed `inlier_threshold` (default 2.0 pixels):

```rust
// Current scoring in count_inliers()
if dist_sq < threshold_sq {
    inliers.push(i);
    score += threshold_sq - dist_sq;
}
```

This threshold requires manual tuning based on seeing conditions:
- For 1.3px FWHM seeing: 2.0px works well
- For 2-3px FWHM seeing: should increase to 3-4px
- Wrong threshold → missed inliers or outlier contamination

## MAGSAC++ Algorithm

### Core Concept

Instead of a binary inlier/outlier decision, MAGSAC++ computes a continuous quality score by marginalizing the point likelihood over a range of noise scales σ ∈ [0, σ_max].

### Mathematical Foundation

For a point with residual r and model θ, the marginal likelihood is:

```
L(r | θ) = ∫₀^σₘₐₓ P(inlier | σ) · P(r | inlier, σ) dσ
```

Under Gaussian noise assumption with χ² distribution for squared residuals:

```
P(r² | σ) = (1 / 2^(k/2) · Γ(k/2)) · (r²/σ²)^(k/2-1) · exp(-r²/(2σ²)) / σ²
```

Where k is the degrees of freedom (k=2 for 2D point correspondences).

### Simplified Scoring Formula

The MAGSAC++ paper derives a closed-form loss function using the incomplete gamma function:

```
loss(r) = {
    outlier_loss,                                           if r > r_max
    C · [σ²_max/2 · γ(k/2, r²/(2σ²_max)) + r²/4 · (Γ(k/2) - γ_k)]   otherwise
}
```

Where:
- `γ(a, x)` = lower incomplete gamma function
- `Γ(a)` = complete gamma function  
- `k` = degrees of freedom (2 for 2D points)
- `σ_max` = maximum expected noise scale
- `C` = normalization constant = 2^(k+1) / σ_max

### Weight Calculation for IRLS

For iteratively reweighted least squares refinement:

```
w(r) = {
    C · 2^(k-1) / σ_max · Γ(k/2),                if r ≈ 0
    C · 2^(k-1) / σ_max · (γ(k/2, x) - γ_k),     otherwise
}
```

Where `x = r² / (2σ²_max)`

## Implementation Design

### 1. Configuration Changes

Replace `inlier_threshold` with `max_sigma` in `RansacParams`:

```rust
// In ransac/mod.rs

pub struct RansacParams {
    pub max_iterations: usize,
    /// Maximum noise scale (σ_max) in pixels.
    /// Points with residuals > k·σ_max are treated as outliers,
    /// where k ≈ 3.03 (√χ²₀.₉₉(2) ≈ √9.21).
    /// 
    /// Default: 1.0 pixel (effective threshold ~3px).
    /// For 2px seeing, use ~0.7. For 4px seeing, use ~1.3.
    pub max_sigma: f64,
    pub confidence: f64,
    pub min_inlier_ratio: f64,
    pub seed: Option<u64>,
    pub use_local_optimization: bool,
    pub lo_max_iterations: usize,
    pub max_rotation: Option<f64>,
    pub scale_range: Option<(f64, f64)>,
}

impl Default for RansacParams {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            max_sigma: 1.0,  // ~3px effective threshold
            confidence: 0.999,
            min_inlier_ratio: 0.3,
            seed: None,
            use_local_optimization: true,
            lo_max_iterations: 10,
            max_rotation: None,
            scale_range: None,
        }
    }
}
```

**Migration:** Code using `inlier_threshold: 2.0` should use `max_sigma: 2.0 / 3.0 ≈ 0.67`.

### 2. Gamma Function Lookup Table

Pre-compute incomplete gamma function values for fast evaluation:

```rust
// In ransac/magsac.rs (new file)

/// Pre-computed incomplete gamma function lookup table.
/// 
/// Stores γ(k/2, x) for x = 0, δ, 2δ, ..., x_max
/// where δ = precision step and x_max = (r_max / σ_max)² / 2
pub struct GammaLut {
    /// Lookup table values: stored_gamma[i] = γ(k/2, i * step)
    values: Vec<f64>,
    /// Step size in x-space
    step: f64,
    /// Inverse step for index calculation
    inv_step: f64,
    /// Degrees of freedom / 2
    k_half: f64,
    /// Complete gamma Γ(k/2)
    gamma_k_half: f64,
}

impl GammaLut {
    /// Create lookup table for 2D point correspondences (k=2).
    /// 
    /// # Arguments
    /// * `max_sigma` - Maximum noise scale
    /// * `precision` - Number of table entries (default: 1024)
    pub fn new_2d(max_sigma: f64, precision: usize) -> Self {
        let k_half = 1.0; // k/2 for k=2
        let gamma_k_half = 1.0; // Γ(1) = 1
        
        // x_max corresponds to r = chi_quantile * sigma_max
        // For 99% quantile of χ²(2), chi_quantile ≈ 9.21 (r/σ ≈ 3.03)
        let chi_quantile_sq = 9.21; // χ²₀.₉₉(2)
        let x_max = chi_quantile_sq / 2.0;
        
        let step = x_max / (precision as f64);
        let inv_step = 1.0 / step;
        
        // Compute table: γ(1, x) = 1 - exp(-x) for k=2
        let values: Vec<f64> = (0..=precision)
            .map(|i| {
                let x = i as f64 * step;
                lower_incomplete_gamma_1(x)
            })
            .collect();
        
        Self {
            values,
            step,
            inv_step,
            k_half,
            gamma_k_half,
        }
    }
    
    /// Lookup γ(k/2, x) with linear interpolation.
    #[inline]
    pub fn lookup(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        
        let idx_f = x * self.inv_step;
        let idx = idx_f as usize;
        
        if idx >= self.values.len() - 1 {
            return self.gamma_k_half; // Saturate at Γ(k/2)
        }
        
        // Linear interpolation
        let t = idx_f - idx as f64;
        self.values[idx] * (1.0 - t) + self.values[idx + 1] * t
    }
}

/// Lower incomplete gamma function γ(1, x) = 1 - exp(-x).
/// Special case for k=2 (degrees of freedom for 2D points).
#[inline]
fn lower_incomplete_gamma_1(x: f64) -> f64 {
    1.0 - (-x).exp()
}
```

### 3. MAGSAC++ Scoring Function

```rust
/// MAGSAC++ scoring state, initialized once per RANSAC run.
pub struct MagsacScorer {
    /// Gamma function lookup table
    gamma_lut: GammaLut,
    /// Maximum sigma squared
    max_sigma_sq: f64,
    /// Outlier loss (assigned to points beyond threshold)
    outlier_loss: f64,
    /// Chi-square quantile for outlier cutoff
    chi_quantile_sq: f64,
    /// Normalization constant
    norm_const: f64,
}

impl MagsacScorer {
    pub fn new(max_sigma: f64) -> Self {
        let max_sigma_sq = max_sigma * max_sigma;
        let chi_quantile_sq = 9.21; // χ²₀.₉₉(2)
        
        // Outlier loss = loss at the boundary (r² = chi_quantile_sq * σ²_max)
        // This ensures continuity at the outlier threshold
        let outlier_loss = max_sigma_sq / 2.0; // Simplified for k=2
        
        // Normalization: 2^(k+1) / σ_max = 8 / σ_max for k=2
        let norm_const = 8.0 / max_sigma;
        
        Self {
            gamma_lut: GammaLut::new_2d(max_sigma, 1024),
            max_sigma_sq,
            outlier_loss,
            chi_quantile_sq,
            norm_const,
        }
    }
    
    /// Compute MAGSAC++ loss for a single point.
    /// Lower loss = better fit.
    #[inline]
    pub fn loss(&self, residual_sq: f64) -> f64 {
        // Outlier threshold: r² > χ²_quantile · σ²_max
        let threshold_sq = self.chi_quantile_sq * self.max_sigma_sq;
        
        if residual_sq > threshold_sq {
            return self.outlier_loss;
        }
        
        // x = r² / (2σ²_max)
        let x = residual_sq / (2.0 * self.max_sigma_sq);
        
        // For k=2: loss = σ²_max/2 · γ(1,x) + r²/4 · (1 - γ(1,x))
        // Simplified: loss = σ²_max/2 · (1-e^(-x)) + r²/4 · e^(-x)
        let gamma_x = self.gamma_lut.lookup(x);
        let one_minus_gamma = 1.0 - gamma_x;
        
        self.max_sigma_sq / 2.0 * gamma_x + residual_sq / 4.0 * one_minus_gamma
    }
    
    /// Compute MAGSAC++ weight for IRLS refinement.
    /// Higher weight = more confidence in being an inlier.
    #[inline]
    pub fn weight(&self, residual_sq: f64) -> f64 {
        let threshold_sq = self.chi_quantile_sq * self.max_sigma_sq;
        
        if residual_sq > threshold_sq {
            return 0.0; // Outliers get zero weight
        }
        
        if residual_sq < 1e-10 {
            // Avoid division by zero; return maximum weight
            return 1.0;
        }
        
        let x = residual_sq / (2.0 * self.max_sigma_sq);
        let gamma_x = self.gamma_lut.lookup(x);
        
        // Weight proportional to derivative of loss w.r.t. model parameters
        // For k=2: w ∝ (1 - γ(1,x)) / r²
        // Normalized to [0, 1] range
        let one_minus_gamma = 1.0 - gamma_x;
        (one_minus_gamma / x.max(1e-6)).min(1.0)
    }
    
    /// Check if a point should be considered an inlier for counting purposes.
    #[inline]
    pub fn is_inlier(&self, residual_sq: f64) -> bool {
        residual_sq <= self.chi_quantile_sq * self.max_sigma_sq
    }
}
```

### 4. Integration with RANSAC Loop

Replace `count_inliers` with MAGSAC++ scoring:

```rust
/// MAGSAC++ scoring function.
/// Returns negative total loss (higher = better model).
#[inline]
fn score_hypothesis(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    scorer: &MagsacScorer,
    inliers: &mut Vec<usize>,
) -> f64 {
    inliers.clear();
    
    let mut total_loss = 0.0f64;
    for (i, (r, t)) in ref_points.iter().zip(target_points.iter()).enumerate() {
        let p = transform.apply(*r);
        let dist_sq = (p - *t).length_squared();
        total_loss += scorer.loss(dist_sq);
        if scorer.is_inlier(dist_sq) {
            inliers.push(i);
        }
    }
    
    // Negate so higher score = better model
    -total_loss
}
```

The `MagsacScorer` is initialized once at the start of `ransac_loop` from `params.max_sigma`.

### 5. Weighted Least Squares Refinement (Optional Enhancement)

For LO-RANSAC with MAGSAC++, use weights in the refinement step:

```rust
/// Weighted least squares transform estimation.
/// 
/// Points are weighted by their MAGSAC++ weight (confidence of being inlier).
pub fn estimate_transform_weighted(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    weights: &[f64],
    transform_type: TransformType,
) -> Option<Transform> {
    // For similarity transform with weights:
    // Minimize Σ w_i · ||T(r_i) - t_i||²
    //
    // This requires weighted centroid and weighted covariance computation.
    // Implementation depends on transform_type.
    
    match transform_type {
        TransformType::Similarity => {
            estimate_similarity_weighted(ref_points, target_points, weights)
        }
        // Other transform types...
        _ => {
            // Fall back to unweighted for now
            estimate_transform(ref_points, target_points, transform_type)
        }
    }
}
```

## File Structure

```
registration/ransac/
├── mod.rs              # Main RANSAC implementation (modified)
├── transforms.rs       # Transform estimation (existing)
├── magsac.rs           # NEW: MAGSAC++ scorer and gamma LUT
└── simd/
    └── ...             # Existing SIMD implementations
```

## API Changes

### Breaking Change

`inlier_threshold` is replaced by `max_sigma`. This is a breaking change but simplifies the API:

```rust
// Old API (removed)
let params = RansacParams {
    inlier_threshold: 2.0,
    ..Default::default()
};

// New API
let params = RansacParams {
    max_sigma: 0.67,  // Equivalent to old threshold ~2.0px
    ..Default::default()
};
```

### Migration Guide

| Old `inlier_threshold` | New `max_sigma` | Effective threshold |
|------------------------|-----------------|---------------------|
| 1.5 | 0.5 | ~1.5px |
| 2.0 | 0.67 | ~2.0px |
| 3.0 | 1.0 | ~3.0px |
| 4.5 | 1.5 | ~4.5px |

The relationship is: `max_sigma ≈ inlier_threshold / 3.0`

## Testing Strategy

### Unit Tests

1. **Gamma LUT accuracy**: Compare against reference implementation (e.g., `statrs` crate)
2. **Scorer consistency**: Verify loss is monotonic with residual
3. **Weight normalization**: Verify weights are in [0, 1]

### Integration Tests

1. **Robustness test**: MAGSAC++ should handle varying noise levels without parameter changes
2. **Accuracy test**: Registration accuracy should be comparable or better than old MSAC
3. **Edge cases**: Very few stars, high outlier ratio, extreme seeing conditions

### Benchmark

```rust
#[bench]
fn bench_magsac_scoring(b: &mut Bencher) {
    // Compare MSAC vs MAGSAC++ scoring overhead
    // Target: < 2x slowdown vs MSAC
}
```

## Performance Considerations

### Overhead

- **Gamma LUT**: ~8KB memory (1024 f64 values)
- **Per-point cost**: ~10 additional floating-point ops vs MSAC
- **Expected slowdown**: 20-50% in scoring (not significant vs I/O and transform estimation)

### SIMD Optimization (Future)

The scoring loop can be SIMD-vectorized:
- Load 4 residuals (AVX2) or 2 (SSE/NEON)
- Parallel LUT lookups via gather or scalar fallback
- Vectorized loss computation

Not critical for initial implementation; RANSAC is already fast enough.

## Migration Path

1. **Phase 1**: Implement MagsacScorer and GammaLut with tests
2. **Phase 2**: Replace count_inliers with MAGSAC++ scoring in ransac_loop
3. **Phase 3**: Replace `inlier_threshold` with `max_sigma` in RansacParams
4. **Phase 4**: Update all callers and tests
5. **Phase 5**: Add weighted refinement for LO-RANSAC
6. **Phase 6**: (Optional) SIMD optimization

## References

- [MAGSAC++, a Fast, Reliable and Accurate Robust Estimator (CVPR 2020)](https://openaccess.thecvf.com/content_CVPR_2020/papers/Barath_MAGSAC_a_Fast_Reliable_and_Accurate_Robust_Estimator_CVPR_2020_paper.pdf)
- [GitHub: danini/magsac](https://github.com/danini/magsac) - Reference C++ implementation
- [OpenCV USAC Documentation](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html) - OpenCV's MAGSAC integration
- [pymagsac](https://github.com/ducha-aiki/pymagsac) - Python bindings

## Appendix: Derivation for k=2

For 2D point correspondences, k=2 (degrees of freedom), which simplifies the formulas:

- Γ(k/2) = Γ(1) = 1
- γ(k/2, x) = γ(1, x) = 1 - e^(-x)
- χ²₀.₉₉(2) = 9.21 (99% quantile)
- χ²₀.₉₅(2) = 5.99 (95% quantile)

The loss function simplifies to:

```
loss(r) = σ²_max/2 · (1 - e^(-x)) + r²/4 · e^(-x)
        where x = r² / (2σ²_max)
```

This can be further simplified:

```
loss(r) = σ²_max/2 - (σ²_max/2 - r²/4) · e^(-r²/(2σ²_max))
```

Which shows:
- As r → 0: loss → 0
- As r → ∞: loss → σ²_max/2 (the outlier loss)
- Smooth transition with no hard threshold
