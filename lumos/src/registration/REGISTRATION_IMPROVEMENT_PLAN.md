# Registration Module Improvement Plan

## Overview

This document outlines remaining improvements for the registration module after completing the initial ALGORITHM_REVIEW milestones (K-d tree, LO-RANSAC, SIMD, TPS, test hardening).

---

## Priority 1: Quick Wins (High Impact, Low Effort)

### 1.1 API Visibility Cleanup

**Goal**: Reduce public API surface by changing `pub` to `pub(crate)` for internal functions.

**Files to modify**:
- `triangle/mod.rs`: `form_triangles()`, `matches_to_point_pairs()` -> `pub(crate)`
- `ransac/mod.rs`: `adaptive_iterations()`, `normalize_points()`, `centroid()`, `compute_residuals()`, `refine_transform()` -> `pub(crate)`
- `ransac/simd/mod.rs`: `count_inliers_scalar()`, `compute_residuals_scalar()` -> private
- `mod.rs`: Remove re-exports of `Triangle`, `KdTree`, keep only high-level API

**Estimated effort**: 30 minutes

---

### 1.2 Extract Shared Constants

**Goal**: Replace magic numbers with named constants.

**Create**: `registration/constants.rs`

```rust
// Numerical precision thresholds
pub const EPSILON: f64 = 1e-10;
pub const SINGULAR_THRESHOLD: f64 = 1e-12;
pub const COLLINEAR_THRESHOLD: f64 = 1e-15;

// Algorithm defaults
pub const DEFAULT_TRIANGLE_TOLERANCE: f64 = 0.01;
pub const DEFAULT_RANSAC_THRESHOLD: f64 = 2.0;
pub const DEFAULT_HASH_BINS: usize = 100;
pub const DEFAULT_MAX_RANSAC_ITERATIONS: usize = 1000;
pub const DEFAULT_RANSAC_CONFIDENCE: f64 = 0.999;

// SIMD dispatch thresholds
pub const SIMD_MIN_POINTS: usize = 8;
```

**Estimated effort**: 1 hour

---

### 1.3 Delete Brute-Force Triangle Matching

**Goal**: remove `match_stars_triangles()` , use `match_stars_triangles_kdtree()`.


**Estimated effort**: 15 minutes

---

### 1.4 Fix Triangle Area Check

**Location**: `triangle/mod.rs:76-79`

**Current** (too permissive):
```rust
if sides[0] + sides[1] <= sides[2] * 1.001 {
    return None;
}
```

**Fixed** (proper area threshold):
```rust
// Use Heron's formula for area
let s = (sides[0] + sides[1] + sides[2]) / 2.0;
let area_sq = s * (s - sides[0]) * (s - sides[1]) * (s - sides[2]);
if area_sq < MIN_TRIANGLE_AREA_SQ {
    return None; // Too flat
}
```

**Estimated effort**: 30 minutes

---

## Priority 2: Code Quality (Medium Impact)

### 2.1 Extract Duplicate Triangle Voting Logic

**Goal**: Consolidate voting logic into shared helper.

**Create helper function**:
```rust
fn vote_for_triangles(
    ref_triangles: &[Triangle],
    target_triangles: &[Triangle],
    hash_table: &TriangleHashTable,
    config: &TriangleMatchConfig,
) -> HashMap<(usize, usize), usize> {
    let mut votes = HashMap::new();
    
    for target_tri in target_triangles {
        let candidates = hash_table.find_candidates(target_tri, config.ratio_tolerance);
        for &ref_idx in &candidates {
            let ref_tri = &ref_triangles[ref_idx];
            if config.check_orientation && ref_tri.orientation != target_tri.orientation {
                continue;
            }
            if ref_tri.is_similar_to(target_tri, config.ratio_tolerance) {
                // Vote for vertex correspondences
                for (r, t) in ref_tri.vertices.iter().zip(target_tri.vertices.iter()) {
                    *votes.entry((*r, *t)).or_insert(0) += 1;
                }
            }
        }
    }
    votes
}
```

**Estimated effort**: 1 hour

---

### 2.2 Extract Conflict Resolution Logic

**Goal**: Consolidate match conflict resolution.

```rust
fn resolve_match_conflicts(
    votes: &HashMap<(usize, usize), usize>,
    n_ref: usize,
    n_target: usize,
    min_votes: usize,
) -> Vec<StarMatch> {
    let mut matches: Vec<_> = votes
        .iter()
        .filter(|(_, &v)| v >= min_votes)
        .map(|(&(r, t), &v)| (r, t, v))
        .collect();
    
    // Sort by votes descending
    matches.sort_by(|a, b| b.2.cmp(&a.2));
    
    // Greedy one-to-one assignment
    let mut used_ref = vec![false; n_ref];
    let mut used_target = vec![false; n_target];
    let mut result = Vec::new();
    
    for (ref_idx, target_idx, votes) in matches {
        if !used_ref[ref_idx] && !used_target[target_idx] {
            used_ref[ref_idx] = true;
            used_target[target_idx] = true;
            result.push(StarMatch {
                ref_idx,
                target_idx,
                confidence: votes as f64 / (votes as f64 + 1.0),
            });
        }
    }
    result
}
```

**Estimated effort**: 1 hour

---

### 2.3 Consolidate Residual Computation

**Goal**: Single implementation used by both RANSAC and quality modules.

**Changes**:
- Keep `ransac/simd/mod.rs::compute_residuals_simd()` as the canonical implementation
- Have `quality/mod.rs::compute_residuals()` call into RANSAC module
- Remove duplicate implementation

**Estimated effort**: 30 minutes

---

## Priority 3: Memory Optimization

### 3.1 Pre-allocate RANSAC Sample Buffers

**Location**: `ransac/mod.rs:164-175`

**Current**:
```rust
let sample_ref: Vec<(f64, f64)> = sample_indices.iter().map(|&i| ref_points[i]).collect();
let sample_target: Vec<(f64, f64)> = sample_indices.iter().map(|&i| target_points[i]).collect();
```

**Optimized** (use stack arrays for small samples):
```rust
// For translation (2 points), similarity (2), euclidean (2), affine (3), homography (4)
let mut sample_ref = [(0.0, 0.0); 8];
let mut sample_target = [(0.0, 0.0); 8];
for (i, &idx) in sample_indices.iter().enumerate() {
    sample_ref[i] = ref_points[idx];
    sample_target[i] = target_points[idx];
}
let sample_ref = &sample_ref[..sample_size];
let sample_target = &sample_target[..sample_size];
```

**Estimated effort**: 1 hour

---

### 3.2 Use Dense Vote Matrix for Small Star Counts

**Location**: `triangle/mod.rs:256`

**Current**:
```rust
let mut vote_matrix: HashMap<(usize, usize), usize> = HashMap::new();
```

**Optimized** (use Vec for N < 1000):
```rust
let use_dense = n_ref * n_target < 1_000_000;
if use_dense {
    let mut votes = vec![0u16; n_ref * n_target];
    // ... direct indexing: votes[ref_idx * n_target + target_idx] += 1;
} else {
    let mut votes: HashMap<(usize, usize), usize> = HashMap::new();
    // ... sparse for huge star counts
}
```

**Estimated effort**: 1.5 hours

---

### 3.3 Reuse Hash Table Candidate Buffer

**Location**: `triangle/mod.rs:265`

**Current**:
```rust
for target_tri in &target_triangles {
    let candidates = hash_table.find_candidates(...); // Allocates each time
}
```

**Optimized**:
```rust
let mut candidates_buffer = Vec::with_capacity(64);
for target_tri in &target_triangles {
    candidates_buffer.clear();
    hash_table.find_candidates_into(target_tri, config.ratio_tolerance, &mut candidates_buffer);
    // ... use candidates_buffer
}
```

**Estimated effort**: 1 hour

---

## Priority 4: Additional SIMD Optimization

### 4.1 SIMD Affine Transform Estimation

**Location**: `ransac/mod.rs:485-550`

**Goal**: Vectorize the 11 accumulations in the inner loop.

```rust
// Current: 11 scalar accumulators
for ((rx, ry), (tx, ty)) in points {
    sum_x += rx; sum_y += ry; sum_xx += rx*rx; ...
}

// SIMD: Process 4 points at once
#[cfg(target_arch = "x86_64")]
unsafe fn accumulate_affine_avx(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
) -> AffineAccumulators {
    // Use _mm256_* intrinsics
}
```

**Estimated effort**: 4 hours

---

### 4.2 SIMD Lanczos Kernel Computation

**Location**: `interpolation/mod.rs:121-135`

**Goal**: Compute 4-8 kernel weights simultaneously.

**Estimated effort**: 3 hours

---

### 4.3 SIMD KdTree Distance Batch

**Location**: `spatial/mod.rs`

**Goal**: When finding k-nearest, compute distances to multiple candidate points simultaneously.

**Estimated effort**: 3 hours

---

## Priority 5: Algorithm Enhancements

### 5.1 Multi-Scale Registration

**Goal**: Speed up large image registration with pyramid approach.

```rust
pub fn register_multiscale(
    ref_image: &[f32],
    target_image: &[f32],
    width: usize,
    height: usize,
    levels: usize,
) -> RegistrationResult {
    let mut current_transform = TransformMatrix::identity();
    
    for level in (0..levels).rev() {
        let scale = 1 << level;
        let scaled_ref = downsample(ref_image, width, height, scale);
        let scaled_target = downsample(target_image, width, height, scale);
        
        // Register at this scale, using previous as initial guess
        let result = register_with_initial(&scaled_ref, &scaled_target, current_transform);
        current_transform = result.transform.scale(scale as f64);
    }
    
    RegistrationResult { transform: current_transform, ... }
}
```

**Estimated effort**: 8 hours

---

### 5.2 Log-Polar Phase Correlation for Rotation

**Goal**: Estimate rotation/scale via FFT before triangle matching.

```rust
pub fn estimate_rotation_scale(
    ref_image: &[f32],
    target_image: &[f32],
    width: usize,
    height: usize,
) -> (f64, f64) {
    // 1. Compute magnitude spectrum (shift-invariant)
    let ref_mag = fft_magnitude_spectrum(ref_image, width, height);
    let target_mag = fft_magnitude_spectrum(target_image, width, height);
    
    // 2. Convert to log-polar coordinates
    let ref_lp = to_log_polar(&ref_mag);
    let target_lp = to_log_polar(&target_mag);
    
    // 3. Phase correlate to find rotation (x) and scale (y)
    let (dx, dy, _) = phase_correlate(&ref_lp, &target_lp);
    
    let rotation = dx * (2.0 * PI / width as f64);
    let scale = (dy * log_base / height as f64).exp();
    
    (rotation, scale)
}
```

**Estimated effort**: 6 hours

---

### 5.3 Guided/Progressive RANSAC

**Goal**: Use match confidence to guide hypothesis sampling.

```rust
fn progressive_ransac(
    matches: &[StarMatch], // With confidence scores
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    config: &RansacConfig,
) -> Option<RansacResult> {
    // Sort matches by confidence
    let sorted: Vec<_> = matches.iter()
        .enumerate()
        .sorted_by(|a, b| b.1.confidence.partial_cmp(&a.1.confidence).unwrap())
        .collect();
    
    // Sample preferentially from high-confidence matches
    for iteration in 0..config.max_iterations {
        let sample = progressive_sample(&sorted, iteration);
        // ... standard RANSAC logic
    }
}
```

**Estimated effort**: 4 hours

---

## Priority 6: Test Improvements

### 6.1 Numerical Stability Tests

```rust
#[test]
fn test_ransac_extreme_scale() {
    // Test with coordinates scaled by 1e6
    let ref_points: Vec<_> = (0..20).map(|i| (i as f64 * 1e6, i as f64 * 1e6)).collect();
    // ...
}

#[test]
fn test_homography_near_singular() {
    // Test with nearly collinear points
}

#[test]
fn test_triangle_very_flat() {
    // Test with triangles having area < 1e-6
}
```

---

### 6.2 Performance Regression Tests

```rust
#[test]
fn test_ransac_performance_budget() {
    let start = Instant::now();
    for _ in 0..100 {
        ransac_estimate(&ref_points, &target_points, &config);
    }
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(500), "RANSAC too slow: {:?}", elapsed);
}
```

---

### 6.3 Integration Tests with Real Data

```rust
#[test]
#[ignore] // Requires test fixtures
fn test_full_pipeline_m31() {
    let ref_image = load_test_image("m31_ref.fits");
    let target_image = load_test_image("m31_target.fits");
    
    let result = Registrator::new(RegistrationConfig::default())
        .register(&ref_stars, &target_stars, width, height)
        .unwrap();
    
    assert!(result.quality.rms_error < 0.5);
    assert!(result.quality.inlier_ratio > 0.9);
}
```

---

## Priority 7: Code Organization

### 7.1 Module Structure Cleanup

Current:
```
registration/
  mod.rs          (re-exports everything)
  types.rs
  triangle/
  ransac/
  ...
```

Proposed:
```
registration/
  mod.rs          (minimal public API)
  types.rs        (public types)
  constants.rs    (shared constants)
  internal/       (implementation details)
    triangle/
    ransac/
    spatial/
    ...
```

---

## Implementation Order

### Phase 1: Quick Cleanup (1-2 days) - COMPLETED
1. [x] 1.1 API visibility
2. [x] 1.2 Constants module
3. [x] 1.3 Deprecate brute-force (kept for tests, pipeline uses kdtree)
4. [x] 1.4 Fix triangle area (Heron's formula)
5. [x] 2.3 Consolidate residuals

### Phase 2: Code Quality (2-3 days) - COMPLETED
1. [x] 2.1 Extract voting logic (`vote_for_correspondences()`)
2. [x] 2.2 Extract conflict resolution (`resolve_matches()`)
3. [x] 3.1 Pre-allocate RANSAC buffers (`random_sample_into()`)
4. [x] 3.3 Reuse candidate buffer (`find_candidates_into()`)

### Phase 3: Performance (3-5 days) - PARTIALLY COMPLETED
1. [x] 3.2 Dense vote matrix (VoteMatrix enum with sparse/dense)
2. [~] 4.1 SIMD affine estimation (skipped - compiler auto-vectorizes, small sample sizes)
3. [~] 4.2 SIMD Lanczos kernel (skipped - sin() doesn't vectorize, small kernels)
4. [~] 4.3 SIMD KdTree distances (skipped - tree traversal dominates, not distance calc)

### Phase 4: Algorithms (5-7 days) - COMPLETED
1. [x] 5.1 Multi-scale registration (MultiScaleRegistrator, pyramid approach)
2. [x] 5.2 Log-polar rotation estimation (LogPolarCorrelator, FullPhaseCorrelator)
3. [x] 5.3 Progressive RANSAC (estimate_progressive with weighted sampling)

### Phase 5: Testing (2-3 days) - PARTIALLY COMPLETED
1. [x] 6.1 Numerical stability tests (extreme scales, small coords, near-singular transforms)
2. [ ] 6.2 Performance regression tests
3. [x] 6.3 Integration tests (dithered exposures, mosaics, field rotation, atmospheric refraction, centroid noise, partial overlap, plate scales)

---

## Metrics to Track

| Metric | Current | Target |
|--------|---------|--------|
| Public API items | ~50 | ~20 |
| Test coverage | ~80% | >90% |
| 1000-star matching time | ~1.2s | <0.5s |
| RANSAC iterations (typical) | ~200 | ~100 |
| Memory allocations per registration | ~500 | <100 |

---

## Summary

The registration module is well-implemented with solid algorithms. The main opportunities are:

1. **API cleanup**: Too many internal functions are public
2. **Code consolidation**: Duplicate logic in triangle matching
3. **Memory efficiency**: Unnecessary allocations in hot paths
4. **Additional SIMD**: Several scalar loops could be vectorized
5. **Algorithm enhancements**: Multi-scale and guided RANSAC would improve speed

Total estimated effort: ~3-4 weeks for all improvements.
