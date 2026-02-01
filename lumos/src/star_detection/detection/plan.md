# Detection Module Improvement Plan

## Completed Optimizations

### 1. Run-Length Encoding (RLE) for CCL - DONE
**Achieved speedup:** ~50% faster (15ms -> 8ms on 6K globular cluster)

Implemented RLE-based CCL:
- Extract runs from mask using word-level bit scanning
- Label runs and merge with overlapping runs from previous row
- Parallel strip processing with RLE-based boundary merging
- Write labels to output buffer in parallel

**Location:** `labels.rs` - `label_mask_sequential()`, `label_mask_parallel()`, `label_strip_rle()`

---

## High-Priority (Detection Quality)

### 3. Adaptive Local Thresholding
**Impact:** Better detection in variable nebulosity

Current single sigma threshold fails when:
- Bright nebulae raise local background
- Dust lanes lower local signal
- Gradient across field

**Options:**
- **Tile-based:** Different sigma per background tile (already have tiles)
- **Sliding window:** Local sigma in NxN neighborhood
- **Hybrid:** Use local noise estimate, not just global

**Implementation:**
- Add `AdaptiveThresholdConfig` with window size
- Modify `create_threshold_mask` to accept local thresholds
- Consider performance impact (may need SIMD optimization)

### 4. 8-Connectivity Option
**Impact:** Fewer fragmented detections for undersampled PSFs

Current 4-connectivity may split:
- Diagonal PSF features
- Stars near Nyquist sampling limit
- Elongated/trailed sources

**Implementation:**
- Add `connectivity: Connectivity` enum (Four, Eight)
- Modify neighbor iteration in `label_pixel()`
- 8-conn checks 8 neighbors vs 4 (2x more work, but often worth it)

---

## Medium-Priority

### 5. Atomic Path Compression
**Expected improvement:** Reduced tree depth, faster merges

Current `atomic_find()` only reads without compression. Adding path compression:
```rust
fn atomic_find_with_compression(labels: &[AtomicU32], mut x: u32) -> u32 {
    let root = atomic_find(labels, x);  // Find root first
    // Compress path
    while labels[x].load(Relaxed) != root {
        let parent = labels[x].swap(root, Relaxed);
        x = parent;
    }
    root
}
```

**Trade-off:** More atomic operations vs shorter trees. Benchmark needed.

### 6. Auto-Estimate FWHM
**Impact:** Better matched filter without manual tuning

Currently `expected_fwhm` is manual config. Auto-estimation:
1. Detect brightest N stars with conservative threshold
2. Fit Gaussian/Moffat to estimate FWHM
3. Use median FWHM for matched filter

**Implementation:**
- Add `auto_fwhm: bool` to config
- Two-pass detection: coarse → estimate FWHM → fine with matched filter
- Cache FWHM estimate for batch processing

### 7. Vectorized Label Flattening
**Expected speedup:** Minor (~5-10% of CCL time)

Final `flatten_labels()` pass could use SIMD:
- Load 4-8 labels at once
- Lookup in parent array (gather operation)
- Store flattened labels

**Implementation:**
- Use `_mm256_i32gather_epi32` on AVX2
- Fallback to scalar for non-AVX2

---

## Priority Matrix

| Improvement | Effort | Impact | Status |
|-------------|--------|--------|--------|
| RLE-based CCL | High | High (perf) | **DONE** (~50% faster) |
| 8-connectivity option | Low | Medium (quality) | Pending |
| Adaptive thresholding | High | High (quality) | Pending |
| Auto-estimate FWHM | Medium | Medium (usability) | Pending |
| Atomic path compression | Low | Low-Medium | Pending |
| Vectorized label flatten | Low | Low | Pending |

---

## Recommendation

Next priority: **8-connectivity option** - low effort, improves quality for undersampled PSFs. Then **Adaptive thresholding** for images with variable nebulosity.
