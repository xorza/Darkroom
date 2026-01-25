# Registration Algorithm Review and Improvement Plan

## Executive Summary

This document provides a comprehensive review of the current registration implementation, comparing it against industry-leading tools (Astroalign, Siril, PixInsight, DeepSkyStacker), identifying gaps, and proposing improvements.

**Current Status**: The implementation covers all major components (triangle matching, RANSAC, phase correlation, interpolation, quality metrics) with solid foundations. Key areas for improvement are algorithmic optimizations and SIMD acceleration.

---

## 1. Current Implementation Analysis

### 1.1 Triangle Matching (`triangle/mod.rs`)

**Current Approach**:
- Brute-force O(n³) triangle formation from all star combinations
- Geometric hashing using side ratios (a/c, b/c) as invariants
- 2D hash table with configurable bins (default: 100)
- Voting system for star correspondences
- Greedy conflict resolution by vote count

**Strengths**:
- ✅ Properly handles scale and rotation invariance
- ✅ Orientation checking for mirrored images
- ✅ Configurable tolerance and vote thresholds
- ✅ Hash table provides O(1) candidate lookup

**Weaknesses**:
- ❌ O(n³) triangle formation limits scalability to ~50-100 stars
- ❌ No k-d tree for spatial queries (Astroalign uses this)
- ❌ No adaptive star selection based on brightness distribution
- ❌ Vertex correspondence assumes sorted side ordering (may cause mismatches)

### 1.2 RANSAC (`ransac/mod.rs`)

**Current Approach**:
- Standard RANSAC with random sampling
- Adaptive iteration count based on inlier ratio
- Supports all 5 transform types (translation, euclidean, similarity, affine, homography)
- Least-squares refinement on inliers
- Deterministic seeding option

**Strengths**:
- ✅ Correct adaptive termination formula
- ✅ Weighted scoring (not just inlier count)
- ✅ Proper DLT implementation for homography
- ✅ Point normalization for numerical stability

**Weaknesses**:
- ❌ No PROSAC (Progressive Sample Consensus) - uses match confidence
- ❌ No LO-RANSAC (Local Optimization) - refines promising hypotheses
- ❌ No MAGSAC (Marginalizing Sample Consensus) - threshold-free
- ❌ Homography solver uses custom inverse iteration instead of proper SVD

### 1.3 Phase Correlation (`phase_correlation/mod.rs`)

**Current Approach**:
- 2D FFT via row-column decomposition using rustfft
- Hann windowing for edge effect reduction
- Normalized cross-power spectrum
- Three sub-pixel methods: parabolic, Gaussian, centroid
- Confidence based on peak-to-secondary-peak ratio

**Strengths**:
- ✅ Correct implementation of phase correlation algorithm
- ✅ Multiple sub-pixel refinement options
- ✅ Windowing reduces spectral leakage
- ✅ Good confidence metric

**Weaknesses**:
- ❌ No log-polar transform for rotation estimation
- ❌ FFT always pads to power-of-2 (inefficient for some sizes)
- ❌ No band-pass filtering for noise reduction

### 1.4 Interpolation (`interpolation/mod.rs`)

**Current Approach**:
- Multiple methods: Nearest, Bilinear, Bicubic (Catmull-Rom), Lanczos-2/3/4
- Separable kernel application
- Optional kernel normalization
- Border value handling

**Strengths**:
- ✅ Correct Lanczos and bicubic kernel implementations
- ✅ Kernel normalization prevents DC shift
- ✅ Full range of quality/speed tradeoffs

**Weaknesses**:
- ❌ No Lanczos clamping to reduce ringing artifacts
- ❌ No Mitchell-Netravali filter (often better than Catmull-Rom)
- ❌ Single-threaded row processing
- ❌ No SIMD optimization yet

### 1.5 Pipeline (`pipeline/mod.rs`)

**Current Approach**:
- Registrator struct with configuration
- Two paths: star-only and phase-correlation-assisted
- Transform composition when using phase correlation
- Quality validation with error thresholds

**Strengths**:
- ✅ Clean separation of star matching and image warping
- ✅ Composable transforms
- ✅ Configuration validation

**Weaknesses**:
- ❌ No iterative refinement loop
- ❌ No fallback strategies on matching failure
- ❌ No progress callbacks for long operations

### 1.6 Quality Metrics (`quality/mod.rs`)

**Current Approach**:
- RMS, max, median error computation
- Weighted quality score
- Quadrant consistency checking
- Overlap estimation

**Strengths**:
- ✅ Comprehensive error statistics
- ✅ Spatial consistency validation
- ✅ Multi-factor quality scoring

**Weaknesses**:
- ❌ No MAD (Median Absolute Deviation) for outlier-robust statistics
- ❌ No distortion map visualization
- ❌ No per-star weight based on brightness

---

## 2. Industry Comparison

### 2.1 Astroalign (Python Reference)

| Feature | Astroalign | Our Implementation | Gap |
|---------|------------|-------------------|-----|
| Star selection | K-d tree spatial queries | Brute-force | **High** |
| Triangle formation | Nearest-neighbor triangles | All combinations | **High** |
| Similarity metric | Side ratios | Side ratios | None |
| Transform type | Similarity only | All 5 types | Better |
| RANSAC | Standard | Standard | None |

**Key Insight**: Astroalign forms triangles only from spatially close stars using k-d tree, dramatically reducing O(n³) to ~O(n·k²) where k is small.

### 2.2 Siril

| Feature | Siril | Our Implementation | Gap |
|---------|-------|-------------------|-----|
| Coarse alignment | Phase correlation | Phase correlation | None |
| Fine alignment | Triangle matching | Triangle matching | None |
| Transform types | Similarity, Affine, Homography | All 5 types | Better |
| Distortion correction | Polynomial | None | **Medium** |
| Drizzle integration | Yes | No | Medium |

### 2.3 PixInsight

| Feature | PixInsight | Our Implementation | Gap |
|---------|------------|-------------------|-----|
| Star matching | Proprietary (superior) | Triangle matching | Unknown |
| Distortion model | Thin-plate splines | None | **High** |
| Sub-pixel accuracy | <0.01 pixel | ~0.1 pixel | **High** |
| Local distortion | Surface splines | None | **High** |

**Key Insight**: PixInsight uses thin-plate splines for local distortion correction, achieving superior accuracy for wide-field images.

### 2.4 DeepSkyStacker

| Feature | DSS | Our Implementation | Gap |
|---------|-----|-------------------|-----|
| Detection | Multiple algorithms | External | N/A |
| Matching | Triangle + voting | Triangle + voting | Similar |
| Stacking modes | Multiple | N/A | N/A |
| Comet mode | Yes | No | N/A |

---

## 3. Test Coverage Analysis

### 3.1 Current Test Coverage

| Module | Unit Tests | Edge Cases | Integration | Fuzz |
|--------|-----------|------------|-------------|------|
| types | ✅ Good | ✅ Good | ✅ | ❌ |
| triangle | ✅ Good | ⚠️ Partial | ⚠️ | ❌ |
| ransac | ✅ Good | ⚠️ Partial | ⚠️ | ❌ |
| phase_correlation | ✅ Good | ⚠️ Partial | ⚠️ | ❌ |
| interpolation | ✅ Good | ⚠️ Partial | ⚠️ | ❌ |
| pipeline | ⚠️ Partial | ⚠️ Partial | ⚠️ | ❌ |
| quality | ✅ Good | ⚠️ Partial | ❌ | ❌ |

### 3.2 Missing Test Cases

**Triangle Matching**:
- [ ] Very dense star fields (>500 stars)
- [ ] Very sparse star fields (<10 stars)
- [ ] Clustered star distributions
- [ ] Non-uniform brightness distributions
- [ ] Edge-heavy star distributions
- [ ] Performance regression tests

**RANSAC**:
- [ ] Adversarial outlier patterns
- [ ] Near-degenerate configurations
- [ ] Transform type auto-detection
- [ ] Numerical stability at extremes
- [ ] Convergence rate tests

**Phase Correlation**:
- [ ] Large translations (>50% overlap)
- [ ] Very small translations (<1 pixel)
- [ ] Images with strong gradients
- [ ] Images with periodic patterns
- [ ] Performance at various FFT sizes

**Interpolation**:
- [ ] Extreme sub-pixel positions
- [ ] Gradient preservation
- [ ] Noise amplification measurement
- [ ] Aliasing artifact detection
- [ ] Comparison against reference implementations

**Pipeline Integration**:
- [ ] Real astronomical images
- [ ] Ground truth validation
- [ ] Multi-frame sequences
- [ ] Error recovery paths
- [ ] Memory usage under stress

---

## 4. Improvement Priorities

### Priority 1: Critical (Accuracy Impact)

1. **K-d Tree for Triangle Formation** [HIGH IMPACT]
   - Reduces complexity from O(n³) to O(n·k²)
   - Enables use of >100 stars
   - Reference: Astroalign uses scipy.spatial.KDTree

2. **LO-RANSAC Implementation** [HIGH IMPACT]
   - Adds local optimization step after finding promising hypothesis
   - Typically improves inlier count by 5-15%
   - Well-documented algorithm

3. **Thin-Plate Spline Distortion** [HIGH IMPACT]
   - Handles local field distortions
   - Critical for wide-field imaging
   - PixInsight's key advantage

### Priority 2: High (Performance Impact)

4. **SIMD Optimizations** [HIGH IMPACT]
   - Distance calculations in triangle formation
   - Residual computation in RANSAC
   - Interpolation kernel application
   - Target: 2-4x speedup

5. **Parallel Processing** [MEDIUM IMPACT]
   - Parallel triangle formation
   - Parallel row interpolation
   - Parallel residual computation

### Priority 3: Medium (Robustness Impact)

6. **PROSAC Sampling** [MEDIUM IMPACT]
   - Uses match confidence to prioritize sampling
   - Faster convergence with good initial matches
   - Drop-in replacement for random sampling

7. **Lanczos Clamping** [LOW-MEDIUM IMPACT]
   - Reduces ringing artifacts at high-contrast edges
   - Simple implementation
   - Improves visual quality

8. **Log-Polar Phase Correlation** [MEDIUM IMPACT]
   - Estimates rotation and scale from FFT
   - Useful for heavily rotated images
   - Can replace/augment triangle matching for coarse alignment

### Priority 4: Low (Nice to Have)

9. **Mitchell-Netravali Filter**
   - Alternative to bicubic with tunable blur/ring tradeoff
   - Simple addition

10. **MAD-based Outlier Detection**
    - More robust than standard deviation
    - Useful for quality metrics

11. **Progress Callbacks**
    - Better user experience for long operations
    - API addition only

---

## 5. Updated Implementation Plan

### Phase 8: Algorithmic Improvements

#### 8.1 K-d Tree Integration
```
Files to create/modify:
- registration/spatial/mod.rs (new)
- registration/spatial/kdtree.rs (new)
- registration/triangle/mod.rs (modify)

Implementation:
1. Implement 2D k-d tree for star positions
2. Query k nearest neighbors for each star
3. Form triangles only from neighbor sets
4. Update triangle matching to use spatial queries
```

#### 8.2 LO-RANSAC
```
Files to modify:
- registration/ransac/mod.rs

Implementation:
1. After finding hypothesis with >threshold inliers
2. Perform iterative least-squares on inlier set
3. Update inlier set with refined model
4. Repeat until convergence (usually 2-3 iterations)
5. Keep best refined model
```

#### 8.3 Thin-Plate Splines
```
Files to create:
- registration/distortion/mod.rs (new)
- registration/distortion/tps.rs (new)

Implementation:
1. TPS kernel: r² * log(r)
2. Build system of equations from control points
3. Solve for TPS coefficients
4. Apply as post-RANSAC refinement
5. Use for warping with local corrections
```

### Phase 9: SIMD Acceleration

#### 9.1 Triangle SIMD
```
Files to create:
- registration/triangle/simd/mod.rs
- registration/triangle/simd/sse.rs
- registration/triangle/simd/neon.rs

Functions:
- compute_distances_simd: Batch distance calculation
- compare_ratios_simd: Batch tolerance checking
```

#### 9.2 RANSAC SIMD
```
Files to create:
- registration/ransac/simd/mod.rs
- registration/ransac/simd/sse.rs
- registration/ransac/simd/neon.rs

Functions:
- compute_residuals_simd: Transform + distance in parallel
- count_inliers_simd: Vectorized threshold comparison
```

#### 9.3 Interpolation SIMD
```
Files to create:
- registration/interpolation/simd/mod.rs
- registration/interpolation/simd/sse.rs
- registration/interpolation/simd/neon.rs

Functions:
- interpolate_row_simd: Process multiple output pixels
- compute_weights_simd: Vectorized kernel computation
```

### Phase 10: Enhanced Testing

#### 10.1 Property-Based Tests
```
Add proptest/quickcheck tests for:
- Triangle ratio invariants
- Transform composition associativity
- Interpolation boundary conditions
- RANSAC convergence properties
```

#### 10.2 Regression Test Suite
```
Create test fixtures:
- Synthetic star fields with known transforms
- Real astronomical images with ground truth
- Edge case images (very dense, very sparse, etc.)
```

#### 10.3 Performance Benchmarks
```
Add criterion benchmarks comparing:
- With/without k-d tree
- With/without LO-RANSAC
- SIMD vs scalar paths
- Different interpolation methods
```

---

## 6. Recommended Test Additions

### Immediate (Add Now)

```rust
// triangle/tests.rs - Add these tests
#[test]
fn test_match_very_dense_field_500_stars() { /* ... */ }

#[test]
fn test_match_sparse_field_10_stars() { /* ... */ }

#[test]
fn test_match_with_40_percent_outliers() { /* ... */ }

#[test]
fn test_vertex_correspondence_correctness() { /* ... */ }

// ransac/tests.rs - Add these tests
#[test]
fn test_ransac_near_degenerate_affine() { /* ... */ }

#[test]
fn test_ransac_numerical_stability_large_coords() { /* ... */ }

#[test]
fn test_ransac_convergence_rate() { /* ... */ }

// interpolation/tests.rs - Add these tests
#[test]
fn test_lanczos_noise_amplification() { /* ... */ }

#[test]
fn test_interpolation_gradient_preservation() { /* ... */ }

#[test]
fn test_bicubic_vs_lanczos_quality() { /* ... */ }

// pipeline/tests.rs - Add these tests  
#[test]
fn test_pipeline_with_ground_truth_synthetic() { /* ... */ }

#[test]
fn test_pipeline_error_recovery() { /* ... */ }

#[test]
fn test_pipeline_memory_pressure() { /* ... */ }
```

---

## 7. Timeline-Free Milestones

The improvements should be implemented in this order:

1. **Milestone A: Spatial Optimization**
   - K-d tree implementation
   - Triangle matching refactor
   - Validation tests

2. **Milestone B: RANSAC Enhancement**
   - LO-RANSAC implementation
   - Optional PROSAC sampling
   - Convergence tests

3. **Milestone C: SIMD Foundation**
   - Distance calculation SIMD
   - Residual computation SIMD
   - Benchmark validation

4. **Milestone D: Distortion Modeling**
   - Thin-plate spline implementation
   - Pipeline integration
   - Wide-field validation

5. **Milestone E: Interpolation Optimization**
   - SIMD interpolation
   - Lanczos clamping
   - Quality comparison tests

6. **Milestone F: Test Hardening**
   - Property-based tests
   - Ground truth test suite
   - Performance regression tests

---

## 8. Conclusion

The current implementation provides a solid foundation with correct algorithms for all major registration components. The main gaps compared to industry leaders are:

1. **Scalability**: O(n³) triangle formation limits star count
2. **Accuracy**: No local distortion correction (thin-plate splines)
3. **Performance**: No SIMD optimization yet
4. **Robustness**: Standard RANSAC without LO refinement

Addressing these gaps in priority order will bring the implementation to industry-competitive levels while maintaining the clean, modular architecture already established.
