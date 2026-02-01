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

### 2. 8-Connectivity Option - DONE
**Impact:** Fewer fragmented detections for undersampled PSFs

Implemented configurable connectivity:
- Added `Connectivity` enum (`Four` default, `Eight`) in `config.rs`
- Added `runs_overlap()` helper function that handles both modes
- 4-conn: `prev.start < curr.end && prev.end > curr.start`
- 8-conn: `prev.start < curr.end + 1 && prev.end + 1 > curr.start`
- Connectivity propagated through parallel strip processing and boundary merging
- Added `from_mask_with_connectivity()` method to `LabelMap`

**Location:** `labels.rs` - `runs_overlap()`, `from_mask_with_connectivity()`

**Tests:** `tests.rs` - `label_map_tests::eight_connectivity` module (9 tests)
- `diagonal_connected` - diagonal line detection
- `anti_diagonal_connected` - anti-diagonal line
- `checkerboard_8conn` - full checkerboard connectivity
- `adjacent_runs_diagonal` - adjacent runs with diagonal touch
- `l_shape_diagonal_gap` - L-shape with diagonal chain
- `parallel_strip_boundary_diagonal` - parallel processing boundary case
- `corner_touch_only` - single pixel diagonal touch
- `horizontal_still_connected` - 4-conn still works
- `vertical_still_connected` - 4-conn still works

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
| 8-connectivity option | Low | Medium (quality) | **DONE** |
| Adaptive thresholding | High | High (quality) | **NEXT** |
| Auto-estimate FWHM | Medium | Medium (usability) | Pending |
| Atomic path compression | Low | Low-Medium | Pending |
| Vectorized label flatten | Low | Low | Pending |

---

## Recommendation

**Next:** Implement **Adaptive local thresholding** - high effort but high impact for images with variable nebulosity (bright nebulae, dust lanes, gradients).

Alternative quick wins:
- **Auto-estimate FWHM** - medium effort, improves usability by removing manual tuning
- **Atomic path compression** - low effort, may improve CCL performance (benchmark needed)
