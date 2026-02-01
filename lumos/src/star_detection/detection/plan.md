# Detection Module Improvement Plan

## High-Priority (Performance)

### 1. Run-Length Encoding (RLE) for CCL
**Expected speedup:** 1.7-1.9x

Current word-level bit scanning is a partial step toward RLE. Full RLE would:
- Encode mask as runs of consecutive 1s
- Merge runs instead of individual pixels
- Better SIMD utilization (process entire runs at once)

**Implementation:**
- Add `RunLengthMask` struct with `(start, length)` pairs per row
- Modify `LabelMap::from_mask` to accept RLE input
- Run merging uses same union-find, but operates on runs

**Reference:** [SIMD RLE CCL algorithms](https://hal.science/hal-02492824)

### 2. Decision Tree for Neighbor Checks
**Expected speedup:** ~2x neighbor access reduction

Wu-Otoo-Suzuki algorithm exploits local topology:
- If top-center neighbor has label, copy it (no union needed)
- Only check other neighbors when necessary
- Reduces average neighbor checks from 4 to ~2

**Implementation:**
- Replace linear neighbor loop with decision tree in `label_pixel()`
- Order: top-center → top-left → left → top-right
- Early exit when label found without union

**Reference:** [Optimizing two-pass CCL](https://www.osti.gov/servlets/purl/887435)

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

| Improvement | Effort | Impact | Recommended Order |
|-------------|--------|--------|-------------------|
| Decision tree neighbors | Medium | High (perf) | 1 |
| 8-connectivity option | Low | Medium (quality) | 2 |
| Adaptive thresholding | High | High (quality) | 3 |
| RLE-based CCL | High | High (perf) | 4 |
| Auto-estimate FWHM | Medium | Medium (usability) | 5 |
| Atomic path compression | Low | Low-Medium | 6 |
| Vectorized label flatten | Low | Low | 7 |

---

## Recommendation

Start with **Decision tree for neighbor checks** - medium effort, high impact, self-contained change in `labels.rs`. Then add **8-connectivity option** as it's low effort and improves quality for certain use cases.
