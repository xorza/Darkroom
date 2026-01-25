# Registration Module Cleanup Plan

## Overview

Comprehensive code review of the registration module (8,444 production lines) identifying cleanup opportunities, optimizations, and simplifications.

---

## Phase 1: Dead/Unused Code Removal

### 1.1 Unused Constants in constants.rs

**File:** `constants.rs`

**Problem:** Several constants are defined but never used anywhere in the codebase:

| Constant | Line | Status |
|----------|------|--------|
| `SIMD_AVX2_MIN_POINTS` | 74 | Unused |
| `SIMD_SSE_MIN_POINTS` | 77 | Unused |
| `SIMD_NEON_MIN_POINTS` | 80 | Unused |
| `MIN_INLIERS_FOR_VALID` | 87 | Unused |
| `MAX_RMS_ERROR_FOR_VALID` | 90 | Unused |
| `MIN_INLIER_RATIO_FOR_VALID` | 93 | Unused |
| `MAX_QUADRANT_RMS_DIFFERENCE` | 96 | Unused |

**Action:** Remove these 7 unused constants (~20 lines)

**Effort:** 15 minutes

### 1.2 Brute-Force Triangle Functions

**File:** `triangle/mod.rs`

**Functions:**
- `form_triangles()` (lines 389-410) - O(n³) brute-force
- `match_stars_triangles()` (lines 419-455) - Brute-force matching
- `matches_to_point_pairs()` (lines 457-475) - Point pair conversion

**Status:** Marked with `#[cfg_attr(not(any(test, feature = "bench")), allow(dead_code))]`

**Problem:** These functions are only used in tests and benchmarks. The production API exports only `match_stars_triangles_kdtree`.

**Decision Required:**
- **Option A:** Keep for benchmark comparisons (kdtree vs brute-force) - document this
- **Option B:** Remove entirely, update tests to use kdtree versions

**Recommendation:** Keep for benchmarking (valuable for regression testing kdtree performance), but add documentation explaining why they exist.

**Effort:** 30 minutes (if keeping), 2 hours (if removing and updating tests)

---

## Phase 2: Code Duplication

### 2.1 Bilinear Sampling Functions

**Locations:**
1. `phase_correlation/mod.rs:906` - `fn bilinear_sample()`
2. `interpolation/simd/mod.rs:136` - `pub fn bilinear_sample()`

**Problem:** Two nearly identical implementations of bilinear sampling.

**Difference:** The phase_correlation version uses `f64` for coordinates, the simd version uses `f32`.

**Action:** Keep both (different precision requirements) but add cross-reference comments explaining why both exist.

**Effort:** 15 minutes

### 2.2 Centroid and Point Normalization

**File:** `ransac/mod.rs`

**Functions:**
- `centroid()` (line ~905) - Computes centroid of point set
- `normalize_points()` (line ~867) - Normalizes points for numerical stability

**Problem:** These are general-purpose geometric utilities hidden inside RANSAC module.

**Action:** These functions are `pub(crate)` and specific to RANSAC's needs. Leave as-is - moving them would add complexity without clear benefit.

**Effort:** None (leave as-is)

---

## Phase 3: API Cleanup

### 3.1 TransformMatrix Redundant Methods

**File:** `types/mod.rs`

**Redundant pairs:**
- `from_translation()` vs `translation()` - Both do the same thing

**Current usage:** `from_translation()` is used 30+ times across tests/production.

**Action:** Keep both - `from_translation()` is more readable in some contexts. They're one-liners with no maintenance burden.

**Effort:** None (leave as-is)

### 3.2 Registrator::with_defaults() - ALREADY REMOVED

**Status:** Already removed in Phase 5.1 of previous plan.

---

## Phase 4: Performance Optimizations

### 4.1 Weighted Sampling in RANSAC

**File:** `ransac/mod.rs:501-535`

**Current Implementation:**
```rust
fn weighted_sample_into(...) {
    let mut items_with_keys: Vec<(usize, f64)> = pool
        .iter()
        .map(|&idx| { ... compute key ... })
        .collect();
    
    // Full sort O(n log n)
    items_with_keys.sort_by(...);
    
    for (idx, _) in items_with_keys.into_iter().take(k) {
        buffer.push(idx);
    }
}
```

**Problem:** Uses full sort O(n log n) when only top k elements are needed.

**Better approach:** Use partial sort or bounded heap for O(n log k).

**However:** The pool size is typically small (limited by `max_stars_for_matching`, default 200), and k is typically 2-4. The current implementation is simple and fast enough in practice.

**Action:** Add a comment explaining the trade-off. Only optimize if benchmarks show this is a bottleneck.

**Effort:** 15 minutes (comment) or 2 hours (optimization)

### 4.2 Vote Matrix Threshold

**File:** `triangle/mod.rs:22-57`, `constants.rs:35`

**Current:** `DENSE_VOTE_THRESHOLD = 250_000`

**Analysis:** For 500x500 stars, dense matrix uses ~500KB. Sparse uses more per-entry but fewer entries.

**Action:** The current threshold seems reasonable. Add a comment documenting the memory trade-off calculation.

**Effort:** 15 minutes

---

## Phase 5: Documentation Improvements

### 5.1 Constants Documentation

**File:** `constants.rs`

**Problem:** Some constants lack explanation of how values were derived.

**Constants needing better docs:**
- `DENSE_VOTE_THRESHOLD = 250_000` - Why 250K? Memory calculation?
- `DEFAULT_HASH_BINS = 100` - Why 100 bins?
- `MIN_TRIANGLE_AREA_SQ = 1e-6` - What does this prevent?

**Action:** Add brief comments explaining derivation or rationale.

**Effort:** 30 minutes

### 5.2 Triangle Module Documentation

**File:** `triangle/mod.rs`

**Problem:** The brute-force functions have `#[cfg_attr(...)]` but no comment explaining why they exist.

**Action:** Add module-level documentation explaining:
- Production uses kdtree version exclusively
- Brute-force versions kept for benchmark comparisons
- Why both exist (O(n³) vs O(n·k²) trade-off)

**Effort:** 15 minutes

---

## Phase 6: Test Cleanup

### 6.1 Triangle Tests Using Brute-Force

**File:** `triangle/tests.rs`

**Problem:** Many tests use `match_stars_triangles()` (brute-force) instead of `match_stars_triangles_kdtree()` (production code).

**Affected tests:**
- Lines 107, 134, 164, 185, 207, 231, 244, 326, 335, 492, 527, 572, 778

**Question:** Is this intentional (testing brute-force) or should tests use the production API?

**Action:** 
1. Keep brute-force tests as they validate the algorithm correctness
2. Add parallel tests using kdtree version to ensure production code is tested
3. Add comment explaining test strategy

**Effort:** 1 hour

---

## Summary

| Phase | Items | Impact | Effort |
|-------|-------|--------|--------|
| 1 | Dead code removal | Clean codebase | 45 min |
| 2 | Code duplication | Documentation | 15 min |
| 3 | API cleanup | None needed | 0 |
| 4 | Performance | Documentation | 30 min |
| 5 | Documentation | Maintainability | 45 min |
| 6 | Test cleanup | Test coverage | 1 hour |

**Total effort: ~3-4 hours**

---

## Recommended Implementation Order

1. **Phase 1.1** - Remove unused constants (quick win, no risk)
2. **Phase 5.1** - Document constants (improves maintainability)
3. **Phase 5.2** - Document triangle module (clarifies design decisions)
4. **Phase 4.1/4.2** - Add performance trade-off comments
5. **Phase 2.1** - Add cross-reference comments for bilinear sampling
6. **Phase 6.1** - Improve test coverage for production code path
7. **Phase 1.2** - Decide on brute-force functions (keep with docs, or remove)

---

## Not Recommended Changes

The following items from the initial audit are **not recommended** for implementation:

1. **Moving `centroid()`/`normalize_points()` to shared module** - Over-engineering for single-use functions

2. **Changing `TransformMatrix::inverse()` from assert to Result** - The assert is intentional per project guidelines (crash on logic errors)

3. **Adding condition number checking to homography** - The SVD implementation already handles ill-conditioned matrices correctly

4. **K-d tree fast-path for small k** - Premature optimization; current implementation is fast enough

5. **Refactoring RegistrationConfigBuilder** - The verbose builder provides good IDE autocomplete; consolidation would reduce discoverability
