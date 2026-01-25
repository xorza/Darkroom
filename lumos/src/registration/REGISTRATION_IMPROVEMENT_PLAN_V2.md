# Registration Module Improvement Plan v2

## Overview

Comprehensive review of the registration module (14,367 LOC) identifying opportunities for:
- Code cleanup and dead code removal
- Test coverage expansion
- Performance optimization
- Algorithm improvements
- API refinement

---

## Phase 1: Code Cleanup (1-2 days) ✅ COMPLETED

**Completed:**
- [x] 1.2 Moved `DENSE_VOTE_THRESHOLD` to `constants.rs`
- [x] 1.3 Fixed vote matrix overflow with `debug_assert!` for saturation detection

**Deferred:**
- 1.1 Dead code removal: `form_triangles()`, `match_stars_triangles()` are used in tests/benchmarks (marked with `#[cfg_attr]`). `transform_point()` is actively used in interpolation SIMD code - not dead code.

### 1.1 Remove Dead Code

**Files to modify:**

| File | Item | Lines | Action |
|------|------|-------|--------|
| `triangle/mod.rs` | `form_triangles()` | 384-407 | Remove (only used in tests, tests should use kdtree) |
| `triangle/mod.rs` | `match_stars_triangles()` | 414-450 | Remove (kdtree version is the public API) |
| `triangle/mod.rs` | `matches_to_point_pairs()` | 452-470 | Keep but make private (used internally) |
| `types/mod.rs` | `transform_point()` | 174-176 | Remove (alias for `apply()`) |

**Effort:** 2 hours

### 1.2 Move Magic Constants to constants.rs

**Constants to move:**

| Current Location | Constant | Value |
|-----------------|----------|-------|
| `triangle/mod.rs:21` | `DENSE_VOTE_THRESHOLD` | 250_000 |
| `phase_correlation/mod.rs` | Various sub-pixel thresholds | scattered |
| `spatial/mod.rs` | Centroid radius | 2 |

**Effort:** 1 hour

### 1.3 Fix Vote Matrix Overflow

**File:** `triangle/mod.rs:47-49`

**Current:**
```rust
votes[ref_idx * *n_target + target_idx] =
    votes[ref_idx * *n_target + target_idx].saturating_add(1);
```

**Issue:** `u16` saturates at 65535, silently losing votes for very dense matches.

**Fix:** Change to `u32` or add overflow assertion:
```rust
let new_val = votes[idx].saturating_add(1);
debug_assert!(new_val < u16::MAX, "Vote overflow detected");
votes[idx] = new_val;
```

**Effort:** 30 minutes

---

## Phase 2: Test Coverage Expansion (3-4 days) ✅ PARTIALLY COMPLETED

### 2.1 Phase Correlation Tests (HIGH PRIORITY) ✅ DONE

**File:** `phase_correlation/tests.rs`

**Added tests:**
- [x] Sub-pixel refinement accuracy (Parabolic, Gaussian, Centroid methods)
- [x] Large translations with wraparound handling
- [x] Rotation estimation accuracy (45°, small angles)
- [x] Scale estimation accuracy (1.2x)
- [x] FullPhaseCorrelator end-to-end tests
- [x] Low SNR / noisy image scenarios
- [x] Edge cases: uniform/DC images, Nyquist checkerboard

**Effort:** 8 hours → Done

### 2.2 Quality Module Tests ✅ DONE

**File:** `quality/tests.rs`

**Added tests:**
- [x] `check_quadrant_consistency()` with genuinely inconsistent data
- [x] `estimate_overlap()` edge cases (0%, 100%, negative coords, rotation)
- [x] Quality score boundary values
- [x] ResidualStats with single-element and identical values

**Effort:** 4 hours → Done

### 2.3 Distortion Module Tests ✅ DONE

**File:** `distortion/tests.rs`

**Added tests:**
- [x] TPS with various regularization values (λ = 0, 1, 10, 100, 1000)
- [x] Nearly collinear points (ill-conditioned)
- [x] Large deformations stress test
- [x] Bending energy properties (identity, translation, rotation)
- [x] Clustered control points
- [x] Non-uniform distortion map

**Effort:** 3 hours → Done

### 2.4 Spatial Module Tests ✅ DONE

**File:** `spatial/tests.rs`

**Added tests:**
- [x] KdTree with duplicate points
- [x] KdTree with 1000 points (performance)
- [x] Degenerate tree structures (horizontal/vertical lines)
- [x] Points at identical coordinates
- [x] Radius search boundary cases
- [x] k=0 and radius=0 edge cases
- [x] Negative coordinates
- [x] Query far from all points

**Effort:** 3 hours → Done

### 2.5 RANSAC Convergence Tests ✅ DONE

**File:** `ransac/tests.rs`

**Added tests:**
- [x] 100% inliers (all perfect) - `test_ransac_100_percent_inliers`
- [x] 0% inliers (pure noise) - `test_ransac_0_percent_inliers_pure_noise`
- [x] Progressive RANSAC with confidence weights - `test_progressive_ransac_uses_weights`
- [x] Adaptive iteration count verification - `test_adaptive_iteration_count`
- [x] Homography nearly degenerate - `test_homography_nearly_degenerate`
- [x] Early termination - `test_ransac_early_termination`
- [x] Minimum points - `test_ransac_minimum_points`

**Effort:** 4 hours → Done

---

## Phase 3: Performance Optimization (2-3 days)

### 3.1 Lanczos Kernel Lookup Table ✅ DONE

**File:** `interpolation/mod.rs`

**Implementation:**
- Added `LanczosLut` struct with pre-computed kernel values
- Uses 1024 samples per unit interval for ~0.001 precision
- Linear interpolation between LUT entries for smooth results
- Lazy initialization via `OnceLock` (no startup cost)
- Separate LUTs for Lanczos2, Lanczos3, Lanczos4
- Falls back to direct computation for non-standard kernel sizes
- Added 6 LUT accuracy tests

**Technical details:**
- LUT resolution: 1024 samples/unit → total entries: a×1024+1 (e.g., 3073 for Lanczos3)
- Memory: ~12KB per LUT (Lanczos3), initialized lazily
- Accuracy: <0.001 max error vs direct computation
- Kernel symmetry exploited (stores only positive x values)

**Expected speedup:** 20-30% for Lanczos interpolation

**Effort:** 4 hours → Done

### 3.2 FFT Transpose Optimization

**File:** `phase_correlation/mod.rs:225-258`

**Current:** 3 full matrix transposes in `fft_2d()`

**Options:**
1. Use cache-oblivious transpose algorithm
2. Pre-transpose reference image once, reuse
3. Consider in-place 2D FFT algorithms

**Effort:** 4 hours

### 3.3 K-d Tree Neighbor Collection

**File:** `spatial/mod.rs:81-115`

**Current:** `BoundedMaxHeap` uses `Vec` even for small k

**Optimization:** For k <= 32, use fixed-size array:
```rust
enum NearestBuffer {
    Small([Option<(f64, usize)>; 32]),
    Large(BinaryHeap<...>),
}
```

**Effort:** 3 hours

---

## Phase 4: Algorithm Improvements (3-4 days)

### 4.1 Replace Iterative Eigensolver for Homography

**File:** `ransac/mod.rs:698-751`

**Current:** `solve_homogeneous_9x9()` uses 50 iterations of power iteration

**Issue:** Slow and potentially inaccurate for ill-conditioned matrices

**Improvement:** Use SVD from `nalgebra` or `ndarray-linalg`:
```rust
use nalgebra::{DMatrix, SVD};

fn solve_homogeneous_svd(a: &[[f64; 9]; 8]) -> Option<[f64; 9]> {
    let mat = DMatrix::from_row_slice(8, 9, &a.iter().flatten().collect::<Vec<_>>());
    let svd = SVD::new(mat, true, true);
    let v = svd.v_t?.transpose();
    // Last column of V is the solution
    Some(v.column(8).as_slice().try_into().ok()?)
}
```

**Effort:** 4 hours (includes adding dependency)

### 4.2 Phase Correlation Wraparound Handling

**File:** `phase_correlation/mod.rs:156-172`

**Issue:** Large translations (> image_size/4) cause FFT wraparound

**Improvement:** Add coarse-to-fine refinement:
```rust
pub fn correlate_large_offset(&self, ...) -> Option<PhaseCorrelationResult> {
    // 1. Downsample both images by 4x
    // 2. Correlate downsampled (handles up to image_size offsets)
    // 3. Refine at full resolution around coarse estimate
}
```

**Effort:** 4 hours

### 4.3 Make Progressive RANSAC Default

**File:** `ransac/mod.rs`

**Current:** `estimate_progressive()` exists but `estimate()` is default

**Change:** When confidences are available, use progressive sampling by default:
```rust
pub fn estimate(&self, ref_points, target_points, transform_type) -> Option<RansacResult> {
    // Use uniform sampling
}

pub fn estimate_with_matches(&self, matches: &[StarMatch], ...) -> Option<RansacResult> {
    // Extract confidences from matches, use progressive
    let confidences: Vec<f64> = matches.iter().map(|m| m.confidence).collect();
    self.estimate_progressive(ref_points, target_points, &confidences, transform_type)
}
```

**Effort:** 2 hours

---

## Phase 5: API Refinement (1 day)

### 5.1 Simplify Registrator Construction

**File:** `pipeline/mod.rs`

**Current:** Three ways to create Registrator:
- `Registrator::new(config)`
- `Registrator::with_defaults()`
- `Registrator::default()`

**Change:** Keep only `new()` and `Default`:
```rust
impl Registrator {
    pub fn new(config: RegistrationConfig) -> Self { ... }
}

impl Default for Registrator {
    fn default() -> Self {
        Self::new(RegistrationConfig::default())
    }
}

// Remove with_defaults()
```

**Effort:** 1 hour

### 5.2 Improve Error Messages

**File:** `types/mod.rs:464-492`

**Current:** `RegistrationError::RansacFailed` has no details

**Improvement:**
```rust
pub enum RegistrationError {
    RansacFailed {
        reason: RansacFailureReason,
        iterations: usize,
        best_inlier_count: usize,
    },
    // ...
}

pub enum RansacFailureReason {
    NoInliersFound,
    DegeneratePointSet,
    SingularMatrix,
    InsufficientInliers { found: usize, required: usize },
}
```

**Effort:** 2 hours

### 5.3 Document Transform Direction

**File:** `types/mod.rs:163-170`

**Current:** `apply()` direction is implicit

**Change:** Add clear documentation:
```rust
/// Apply transform to map a point from REFERENCE coordinates to TARGET coordinates.
///
/// Given a transform T estimated from `register_stars(ref_stars, target_stars)`:
/// - `T.apply(ref_point)` gives the corresponding target point
/// - `T.apply_inverse(target_point)` gives the corresponding reference point
///
/// For image warping (aligning target to reference frame), use `apply_inverse`.
pub fn apply(&self, x: f64, y: f64) -> (f64, f64) { ... }
```

**Effort:** 30 minutes

---

## Phase 6: Benchmarks (1 day)

### 6.1 Add Performance Regression Benchmarks

**File:** `registration/bench/mod.rs` (new)

```rust
use criterion::{criterion_group, Criterion, BenchmarkId};

fn bench_triangle_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("triangle_matching");
    for n_stars in [50, 100, 200, 500] {
        group.bench_with_input(
            BenchmarkId::new("kdtree", n_stars),
            &n_stars,
            |b, &n| {
                let (ref_stars, target_stars) = generate_test_stars(n);
                b.iter(|| match_stars_triangles_kdtree(&ref_stars, &target_stars, &config));
            },
        );
    }
    group.finish();
}

fn bench_ransac(c: &mut Criterion) { ... }
fn bench_interpolation(c: &mut Criterion) { ... }
fn bench_phase_correlation(c: &mut Criterion) { ... }
```

**Effort:** 4 hours

### 6.2 Document Performance Baselines

Create `PERFORMANCE_BASELINES.md`:
```markdown
# Performance Baselines

Measured on: AMD CPU, 64GB RAM

| Operation | Input Size | Time | Throughput |
|-----------|------------|------|------------|
| Triangle matching (kdtree) | 200 stars | 15ms | - |
| RANSAC (similarity) | 100 matches | 2ms | - |
| Lanczos3 warp | 4K image | 120ms | 138 MP/s |
| Phase correlation | 1024x1024 | 8ms | - |
```

**Effort:** 2 hours

---

## Summary

| Phase | Description | Effort | Impact |
|-------|-------------|--------|--------|
| 1 | Code Cleanup | 3.5h | Maintainability |
| 2 | Test Coverage | 22h | Confidence, Bug Prevention |
| 3 | Performance | 11h | 10-30% faster interpolation |
| 4 | Algorithms | 10h | Robustness, Speed |
| 5 | API Refinement | 3.5h | Usability |
| 6 | Benchmarks | 6h | Regression Prevention |

**Total: ~56 hours (7-8 working days)**

---

## Implementation Order

### Week 1: Foundation
1. [1.1] Remove dead code
2. [1.2] Move constants
3. [1.3] Fix vote overflow
4. [2.1] Phase correlation tests (partial)

### Week 2: Testing
5. [2.1] Phase correlation tests (complete)
6. [2.2] Quality module tests
7. [2.3] Distortion tests
8. [2.4] Spatial tests
9. [2.5] RANSAC tests

### Week 3: Performance & Algorithms
10. [3.1] Lanczos LUT
11. [4.1] SVD for homography
12. [4.2] Phase correlation wraparound
13. [4.3] Progressive RANSAC default

### Week 4: Polish
14. [3.2] FFT transpose (if time)
15. [3.3] KdTree optimization (if time)
16. [5.1-5.3] API refinements
17. [6.1-6.2] Benchmarks

---

## Dependencies

For Phase 4.1 (SVD), consider adding:
```toml
[dependencies]
nalgebra = "0.32"  # or ndarray-linalg with openblas
```

Alternative: Keep manual implementation but improve convergence criterion.
