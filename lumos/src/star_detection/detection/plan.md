# Detection Module Improvement Plan

## Completed Optimizations

### 1. Run-Length Encoding (RLE) for CCL - DONE
**Achieved speedup:** ~50% faster (15ms -> 8ms on 6K globular cluster)

Implemented RLE-based CCL:
- Extract runs from mask using word-level bit scanning
- Label runs and merge with overlapping runs from previous row
- Parallel strip processing with RLE-based boundary merging
- Write labels to output buffer in parallel
- Added 9 RLE-specific tests for correctness verification

**Location:** `labels.rs` - `label_mask_sequential()`, `label_mask_parallel()`, `label_strip_rle()`

**Tests:** `tests.rs` - `label_map_tests::rle_specific` module

---

## Next Up: 8-Connectivity Option

### Why 8-Connectivity?
**Impact:** Fewer fragmented detections for undersampled PSFs

Current 4-connectivity may split:
- Diagonal PSF features
- Stars near Nyquist sampling limit
- Elongated/trailed sources
- Slightly defocused or seeing-affected stars

### Implementation Plan

1. **Add connectivity enum:**
```rust
#[derive(Debug, Clone, Copy, Default)]
pub enum Connectivity {
    #[default]
    Four,
    Eight,
}
```

2. **Modify RLE run overlap check:**
   - 4-conn: runs overlap if `prev.start < curr.end && prev.end > curr.start`
   - 8-conn: runs overlap if `prev.start < curr.end + 1 && prev.end + 1 > curr.start`
   (adjacent runs at diagonal are connected)

3. **Add config option:**
```rust
pub struct StarDetectionConfig {
    // ...
    pub connectivity: Connectivity,
}
```

4. **Update tests** to cover 8-connectivity cases

**Effort:** Low (mostly changing overlap condition)
**Files:** `labels.rs`, `config.rs`, `tests.rs`

---

## Pending Improvements

### Adaptive Local Thresholding
**Impact:** Better detection in variable nebulosity

Current single sigma threshold fails when:
- Bright nebulae raise local background
- Dust lanes lower local signal
- Gradient across field

**Options:**
- **Tile-based:** Different sigma per background tile (already have tiles)
- **Sliding window:** Local sigma in NxN neighborhood
- **Hybrid:** Use local noise estimate, not just global

**Effort:** High

### Auto-Estimate FWHM
**Impact:** Better matched filter without manual tuning

Currently `expected_fwhm` is manual config. Auto-estimation:
1. Detect brightest N stars with conservative threshold
2. Fit Gaussian/Moffat to estimate FWHM
3. Use median FWHM for matched filter

**Effort:** Medium

### Atomic Path Compression
**Expected improvement:** Reduced tree depth, faster merges

Current `atomic_find()` only reads without compression. Adding path compression could reduce tree depth but adds more atomic operations.

**Effort:** Low (benchmark needed to verify benefit)

### Vectorized Label Flattening
**Expected speedup:** Minor (~5-10% of CCL time)

Use SIMD gather operations for label mapping pass.

**Effort:** Low

---

## Priority Matrix

| Improvement | Effort | Impact | Status |
|-------------|--------|--------|--------|
| RLE-based CCL | High | High (perf) | **DONE** (~50% faster) |
| 8-connectivity option | Low | Medium (quality) | **NEXT** |
| Adaptive thresholding | High | High (quality) | Pending |
| Auto-estimate FWHM | Medium | Medium (usability) | Pending |
| Atomic path compression | Low | Low-Medium | Pending |
| Vectorized label flatten | Low | Low | Pending |

---

## Recommendation

**Next:** Implement **8-connectivity option** - low effort, improves quality for undersampled PSFs and diagonal star features.

After that: **Adaptive thresholding** for images with variable nebulosity (higher effort but high impact for real-world astrophotography).
