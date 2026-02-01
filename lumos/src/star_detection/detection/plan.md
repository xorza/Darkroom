# Detection Module Improvement Plan

## Completed Optimizations

### 1. RLE-based Connected Component Labeling ✓
**Achieved:** ~50% faster (15ms → 8ms → 5.5ms on 6K globular cluster)

- Run extraction using word-level bit scanning (64-bit fast-paths)
- CTZ-based scanning for mixed words (10x faster for sparse masks)
- Label runs and merge overlapping runs from previous row
- Parallel strip processing with atomic boundary merging
- Lock-free CAS-based union-find for thread safety

**Location:** `labeling/mod.rs`

### 2. 8-Connectivity Option ✓
**Impact:** Fewer fragmented detections for undersampled PSFs

- `Connectivity` enum (`Four` default, `Eight`) in `config.rs`
- `Run::search_window()` method handles connectivity-specific overlap
- `runs_connected()` function for adjacency checks

### 3. Code Organization ✓
Clean module structure with comprehensive tests (59 tests).

---

## Investigated but Not Beneficial

These optimizations were tested and showed no improvement:

| Optimization | Why Not Beneficial |
|--------------|-------------------|
| Atomic path compression | Strip-based processing keeps trees shallow |
| SIMD run extraction | Most words are zeros (scalar fast-path), dispatch overhead |
| Precomputed lookup tables | CTZ already 10x faster for sparse masks |

---

## Pending Improvements

### 1. Adaptive Local Thresholding
**Impact:** High (detection quality)
**Effort:** High

Current single sigma threshold fails when:
- Bright nebulae raise local background
- Dust lanes lower local signal
- Gradient across field

**Options:**
- **Tile-based:** Different sigma per background tile (already have tiles)
- **Sliding window:** Local sigma in NxN neighborhood
- **Hybrid:** Use local noise estimate, not just global

### 2. Auto-Estimate FWHM
**Impact:** Medium (usability)
**Effort:** Medium

Currently `expected_fwhm` is manual config. Auto-estimation:
1. Detect brightest N stars with conservative threshold
2. Fit Gaussian/Moffat to estimate FWHM
3. Use median FWHM for matched filter

### 3. SIMD Label Flattening
**Impact:** Low (performance)
**Effort:** Low

The final label mapping pass could use SIMD for vectorized lookup.

---

## Priority Matrix

| Improvement | Effort | Impact | Status |
|-------------|--------|--------|--------|
| RLE-based CCL | High | High (perf) | **Done** (~50% faster) |
| CTZ run extraction | Medium | High (perf) | **Done** (10x for sparse) |
| 8-connectivity option | Low | Medium (quality) | **Done** |
| Code organization | Low | Medium (maintainability) | **Done** |
| Atomic path compression | Low | - | **Tested, no benefit** |
| SIMD run extraction | Medium | - | **Tested, no benefit** |
| Adaptive thresholding | High | High (quality) | Pending |
| Auto-estimate FWHM | Medium | Medium (usability) | Pending |
| SIMD label flattening | Low | Low (perf) | Pending |

---

## Recommendation

**Next steps in priority order:**

1. **Adaptive local thresholding** - High effort but high impact for images with variable nebulosity. This is the main remaining quality improvement.

2. **Auto-estimate FWHM** - Medium effort, removes need for manual tuning. Good usability win.

3. **SIMD label flattening** - Low effort, minor performance gain. Quick win if needed.

The CCL labeling is now well-optimized. Further performance gains would come from:
- Reducing work in `extract_candidates()` (currently ~200ms for dense fields)
- Optimizing deblending for crowded regions
- Profile-guided optimization of hot paths
