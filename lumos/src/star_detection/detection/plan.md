# Detection Module Improvement Plan

## Completed Optimizations

### 1. RLE-based Connected Component Labeling - DONE
**Achieved speedup:** ~50% faster (15ms -> 8ms on 6K globular cluster)

Implemented RLE-based CCL with union-find:
- Run extraction using word-level bit scanning (64-bit fast-paths)
- Label runs and merge overlapping runs from previous row
- Parallel strip processing with atomic boundary merging
- Lock-free CAS-based union-find for thread safety

**Location:** `labeling/mod.rs`
- `UnionFind` struct - sequential union-find with path compression
- `AtomicUnionFind` struct - lock-free parallel union-find
- `label_mask_sequential()` - for images < 100k pixels
- `label_mask_parallel()` - strip-based parallel algorithm

**Tests:** `labeling/tests.rs` - 45 tests total
- Basic shapes, edge cases, word boundaries
- Parallel processing (strip boundaries, large images)
- RLE-specific (runs, merging, boundaries)
- 8-connectivity (9 tests)

---

### 2. 8-Connectivity Option - DONE
**Impact:** Fewer fragmented detections for undersampled PSFs

Implemented configurable connectivity:
- `Connectivity` enum (`Four` default, `Eight`) in `config.rs`
- `Run::search_window()` method handles connectivity-specific overlap
- `runs_connected()` function for adjacency checks
- Propagated through parallel strip processing

**API:**
```rust
LabelMap::from_mask(&mask)  // 4-connectivity (default)
LabelMap::from_mask_with_connectivity(&mask, Connectivity::Eight)
```

---

### 3. Code Organization - DONE
Moved labeling to separate module with clean structure:

```
detection/
├── mod.rs              # detect_stars(), extract_candidates()
├── tests.rs            # Detection tests
├── bench.rs            # Detection benchmarks
└── labeling/
    ├── mod.rs          # LabelMap, UnionFind, AtomicUnionFind
    ├── tests.rs        # 45 labeling tests
    ├── bench.rs        # Labeling benchmarks
    └── README.md       # Algorithm documentation
```

---

## Pending Improvements

### Adaptive Local Thresholding
**Impact:** Better detection in variable nebulosity
**Effort:** High

Current single sigma threshold fails when:
- Bright nebulae raise local background
- Dust lanes lower local signal
- Gradient across field

**Options:**
- **Tile-based:** Different sigma per background tile (already have tiles)
- **Sliding window:** Local sigma in NxN neighborhood
- **Hybrid:** Use local noise estimate, not just global

### Auto-Estimate FWHM
**Impact:** Better matched filter without manual tuning
**Effort:** Medium

Currently `expected_fwhm` is manual config. Auto-estimation:
1. Detect brightest N stars with conservative threshold
2. Fit Gaussian/Moffat to estimate FWHM
3. Use median FWHM for matched filter

### Atomic Path Compression
**Expected improvement:** Reduced tree depth, faster merges
**Effort:** Low (benchmark needed to verify benefit)

Current `AtomicUnionFind::find()` only reads without compression. Adding atomic path compression could reduce tree depth but adds more atomic operations.

### SIMD RLE Extraction
**Expected speedup:** ~5x for run extraction (based on research)
**Effort:** Medium

Use AVX2/NEON for parallel bit scanning and run extraction.

---

## Priority Matrix

| Improvement | Effort | Impact | Status |
|-------------|--------|--------|--------|
| RLE-based CCL | High | High (perf) | **DONE** (~50% faster) |
| 8-connectivity option | Low | Medium (quality) | **DONE** |
| Code organization | Low | Medium (maintainability) | **DONE** |
| Adaptive thresholding | High | High (quality) | **NEXT** |
| Auto-estimate FWHM | Medium | Medium (usability) | Pending |
| Atomic path compression | Low | Low-Medium | Pending |
| SIMD RLE extraction | Medium | Medium (perf) | Pending |

---

## Recommendation

**Next:** Implement **Adaptive local thresholding** - high effort but high impact for images with variable nebulosity (bright nebulae, dust lanes, gradients).

Alternative quick wins:
- **Auto-estimate FWHM** - medium effort, improves usability by removing manual tuning
- **Atomic path compression** - low effort, may improve CCL performance (benchmark needed)
