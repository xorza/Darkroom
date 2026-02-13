# Detection Pipeline: Threshold Mask, Labeling, Mask Dilation

These three modules form the core detection pipeline used by
`detector/stages/detect.rs` (line 50-136). The pipeline flow is:

1. **Threshold mask** -- mark pixels above `bg + sigma * noise`
2. **Mask dilation** -- expand mask by radius to connect nearby fragments
3. **Connected component labeling** -- group connected pixels into regions

After labeling, regions are filtered by area and edge margin, then passed to
deblending and centroid stages.

---

## Module 1: Threshold Mask (`threshold_mask/`)

**Files:** `mod.rs` (319 lines), `sse.rs` (125 lines), `neon.rs` (133 lines),
`tests.rs` (834 lines), `bench.rs` (87 lines)

### Threshold Formula

Per-pixel local threshold (mod.rs line 52):

```
threshold = bg[i] + sigma_threshold * max(noise[i], 1e-6)
pixel_above = pixels[i] > threshold    (strict greater-than)
```

For filtered (background-subtracted) images (mod.rs line 85):

```
threshold = sigma_threshold * max(noise[i], 1e-6)
pixel_above = filtered[i] > threshold
```

The noise floor clamp of 1e-6 prevents division-by-zero in regions with
zero estimated noise (tests.rs line 188-200).

### Comparison with SExtractor

SExtractor's DETECT_THRESH uses the same formula: a pixel is detected when
`pixel > background + DETECT_THRESH * sigma`. The default sigma_threshold
of 4.0 (config.rs line 274) is slightly more conservative than SExtractor's
typical default of 1.5-2.0 sigma, but the implementation also supports a
matched filter (convolution with PSF kernel) that effectively raises the SNR
of point sources before thresholding, making the higher threshold appropriate.

Key differences from SExtractor:

- **Local threshold**: Both use per-pixel background and noise maps for local
  thresholding, which is critical for images with variable background
  (nebulosity, vignetting). This is equivalent to SExtractor with a
  WEIGHT_TYPE MAP_WEIGHT configuration.
- **Two variants**: Scenarium provides both the standard threshold
  (`create_threshold_mask`, mod.rs line 247) and a filtered variant
  (`create_threshold_mask_filtered`, mod.rs line 289) for pre-convolved
  images. SExtractor only applies filtering before thresholding.
- **Strict greater-than**: Uses `>` not `>=` (mod.rs line 54). This matches
  SExtractor's convention and prevents false detections at exact threshold
  (tests.rs line 203-216).
- **Default threshold**: 4.0 sigma (config.rs line 274) vs SExtractor's
  typical 1.5 sigma, compensated by matched filtering when FWHM is known.

### Output Format

Results are stored in `BitBuffer2` -- a bit-packed buffer using u64 words with
row alignment. Each pixel occupies 1 bit, giving 8x memory reduction over
byte masks. Words are written in parallel per-row using `par_chunks_mut`
(mod.rs line 265-279).

### SIMD Implementations

All SIMD paths process 64 pixels per u64 word, in groups of 4 floats:

**SSE4.1** (sse.rs):
- Processes 16 groups of 4 pixels per word (sse.rs line 31-44)
- `_mm_loadu_ps` for unaligned loads of pixel, bg, noise
- `_mm_max_ps` for noise floor clamping (line 38)
- `_mm_add_ps` + `_mm_mul_ps` for threshold computation (line 39)
- `_mm_cmpgt_ps` for comparison, `_mm_movemask_ps` for 4-bit extraction (lines 40-41)
- Bits accumulated into u64 via `mask << (group * 4)` (line 43)
- Scalar fallback for remainder (last word, lines 47-64)

**NEON** (neon.rs):
- Same structure using `vld1q_f32`, `vmaxq_f32`, `vcgtq_f32`
- Uses `vmlaq_f32` fused multiply-add for `bg + sigma * noise` (line 38)
- Manual bit extraction via `vgetq_lane_u32` (lines 42-45)
- Scalar fallback for remainder words (lines 51-68)

**Dispatch** (mod.rs line 98-168):
- x86_64: runtime check `cpu_features::has_sse4_1()`, falls back to scalar
- aarch64: NEON always available, no runtime check needed
- Other architectures: scalar only

### Benchmarks

Bench file (bench.rs) measures 4K x 4K (16M pixels) for scalar vs SIMD,
both standard and filtered variants. The SIMD path processes 4 pixels per
cycle vs 1 for scalar.

---

## Module 2: Connected Component Labeling (`labeling/`)

**Files:** `mod.rs` (804 lines), `tests.rs` (1790 lines), `bench.rs` (186 lines)

### Algorithm

RLE-based connected component labeling with union-find. This is a
well-established approach described in the literature as combining the
efficiency of run-length encoding with union-find for label equivalence
resolution.

The algorithm has two code paths selected by image size:

- **Sequential** (mod.rs line 306-375): for images < 65,000 pixels
- **Parallel** (mod.rs line 392-473): for images >= 65,000 pixels

The threshold of 65,000 pixels was determined empirically by benchmark
(bench.rs line 93-185, `bench_threshold_sweep`).

### Run-Length Encoding

Runs are extracted from the bit-packed mask using word-level scanning
(mod.rs line 77-133, `extract_runs_from_row`):

1. **All-zero word** (line 91): skip entire 64-pixel block, close any open run
2. **All-ones word** (line 105): extend current run or start new one
3. **Mixed word** (line 114): use CTZ (count trailing zeros) based scanning
   via `extract_runs_from_mixed_word` (lines 137-193)

The CTZ approach for mixed words avoids bit-by-bit iteration. It alternates
between finding the next 1-bit (run start) and next 0-bit (run end) using
`trailing_zeros()` on the word or its complement.

Each Run stores `(start, end, label)` where start is inclusive and end is
exclusive (mod.rs line 44-49).

### Connectivity

Supports both 4-connectivity and 8-connectivity (config.rs line 16-28):

- **4-connectivity** (default): runs overlap if `prev.start < curr.end && prev.end > curr.start` (mod.rs line 67)
- **8-connectivity**: runs overlap if `prev.start < curr.end + 1 && prev.end + 1 > curr.start` (mod.rs line 68)

The `search_window` method (mod.rs line 55-60) expands the search range by 1
pixel on each side for 8-connectivity to catch diagonal adjacency.

**Comparison with SExtractor**: SExtractor uses 8-connectivity by default
("pixels whose values exceed the local threshold and which touch each other
at their sides or angles"). This implementation defaults to 4-connectivity
(config.rs line 275), which avoids merging diagonally-adjacent close star
pairs. The 8-connectivity option is available for wide-field and crowded-field
presets (config.rs lines 464, 503, 528).

**Comparison with photutils**: Python's photutils `detect_sources` defaults
to 8-connectivity as well, matching SExtractor's convention.

### Sequential Path

`label_mask_sequential` (mod.rs line 306-375):

1. Extract runs row by row (line 322-328)
2. For each run, scan previous row's runs for overlaps (lines 336-358)
3. Assign label from first overlapping run, union subsequent overlaps
4. If no overlap, create new label via `uf.make_set()` (line 362)
5. Write labels to output buffer immediately (lines 365-368)
6. After all rows: flatten labels to sequential 1..N (line 374)

The sequential union-find (mod.rs line 605-709) uses:
- Path compression in `find` (two-pass: find root, then compress, lines 628-663)
- Union by smaller root (line 671-677)
- Single-pass flattening (lines 681-708)

### Parallel Path

`label_mask_parallel` (mod.rs line 392-473):

**Phase 1 -- Strip labeling** (lines 411-430):
- Divide image into horizontal strips (min 64 rows each, max num_threads strips)
- Each strip labeled independently using `label_strip` (lines 476-551)
- Uses `AtomicUnionFind` for lock-free label allocation

**Phase 2 -- Boundary merging** (lines 433-439):
- O(n+m) sorted merge of boundary runs between adjacent strips
- `merge_strip_boundary_sorted` (lines 554-598) uses dual-pointer scan

**Phase 3 -- Label map construction** (lines 448):
- `build_label_map` creates sequential label mapping (lines 789-803)

**Phase 4 -- Parallel label writing** (lines 451-465):
- Each strip's runs written in parallel using raw pointer for disjoint access

### Atomic Union-Find

`AtomicUnionFind` (mod.rs line 716-804):
- Lock-free using `AtomicU32` parent array
- `make_set`: `fetch_add` with SeqCst for label allocation (line 731)
- `find`: chase parent pointers with Relaxed loads (lines 739-752)
- `union`: CAS loop with `compare_exchange_weak` (lines 754-781)
  - Always points larger root to smaller root (deterministic)
  - Retries on CAS failure with re-found roots

Pre-allocated with 5% of pixel count as upper bound (line 408), which is
generous for typical astronomical masks (1-3% foreground).

### Comparison with Industry Standards

This implementation combines several best practices from the CCL literature:

1. **RLE + union-find**: matches the approach described by He et al. (2017)
   "The connected-component labeling problem: A review of state-of-the-art
   algorithms" as among the fastest approaches.

2. **Block-parallel with boundary merge**: follows the strategy from Stava &
   Benes (2011) and similar parallel CCL papers that divide the image into
   strips, label independently, then merge at boundaries.

3. **Word-level bit scanning**: exploits the bit-packed mask format for
   efficient RLE extraction, skipping 64 background pixels at a time.

4. **Lock-free atomic union-find**: avoids mutex contention during parallel
   strip labeling, similar to the PAREMSP algorithm.

Differences from classical two-pass pixel-wise CCL:
- Operates on runs rather than individual pixels, reducing label operations
- Word-level scanning skips large background regions efficiently
- Separates label allocation (parallel-safe) from label resolution

### Tests

Comprehensive test suite (1790 lines) organized into:

- Basic shapes: single pixel, lines, L-shape, U-shape (lines 11-172)
- Word boundaries: 64-pixel and 128-pixel word crossings (lines 263-319)
- Parallel path: strip boundary merging, vertical lines, U-shapes, many components (lines 321-628)
- RLE-specific: long runs, multiple runs per row, mixed words (lines 631-903)
- 8-connectivity: diagonal, anti-diagonal, checkerboard, parallel strip boundary (lines 906-1116)
- Ground truth: flood-fill reference implementation for both 4-conn and 8-conn (lines 1122-1207)
- Property-based: random sparse/dense/large masks compared against reference (lines 1528-1628)
- Pixel-level: exact label value verification (lines 1630-1789)

The `verify_ccl_invariants` function (lines 1210-1293) checks four invariants:
1. Background pixels have label 0
2. Foreground pixels have non-zero labels
3. Connected neighbors have the same label
4. Labels are sequential 1..N

### Benchmarks

- 1K x 1K with 500 stars: `bench_label_map_from_buffer_1k` (bench.rs line 38)
- 4K x 4K with 2000 stars: `bench_label_map_from_buffer_4k` (bench.rs line 56)
- 4K x 4K with 50000 stars (globular): `bench_label_map_from_buffer_6k_globular` (bench.rs line 74)
- Threshold sweep: sequential vs parallel timing at various sizes (bench.rs line 93)

---

## Module 3: Mask Dilation (`mask_dilation/`)

**Files:** `mod.rs` (237 lines), `tests.rs` (829 lines), `bench.rs` (28 lines)

### Purpose

Mask dilation serves two purposes in the pipeline:

1. **Detection dilation** (detect.rs line 115): radius 1 dilation after
   thresholding, before labeling. Connects nearby above-threshold pixels that
   might be separated due to noise fluctuations at the detection boundary.
   This prevents a single star from being fragmented into multiple small
   components.

2. **Background mask dilation** (background/mod.rs line 141): configurable
   radius (`bg_mask_dilation`, default 3, config.rs line 196) to mask object
   wings during iterative background refinement. Ensures the background
   estimate is not contaminated by faint star halos.

### Structuring Element

The dilation uses a **square (box) structuring element** of size
`(2*radius+1) x (2*radius+1)`, implemented as separable horizontal + vertical
passes (mod.rs line 22, doc comment).

This is NOT a disk/circular structuring element. The separability means
complexity is O(width*height) rather than O(width*height*radius^2) for a
naive 2D approach. The square SE is standard for astronomical detection
pipelines -- SExtractor also uses rectangular growth for detection masks.

### Algorithm

**Horizontal pass** (mod.rs lines 36-64):
- Parallel over rows via `into_par_iter` (line 36)
- Two implementations based on radius:
  - `dilate_word_fast` (lines 149-179): radius <= 63, uses bit shift smearing
    within and across adjacent u64 words
  - `dilate_word_slow` (lines 184-203): radius > 63, per-bit range check
    using `has_set_bit_in_range` with word-level masks (lines 207-236)
- Last word masked to width boundary (lines 56-60)

**Vertical pass** (mod.rs lines 67-100):
- Column-chunked parallelism (chunk size >= 64 words)
- Reads column data into contiguous buffer (line 88-89)
- `dilate_column_sliding` (lines 110-144): sliding window OR with lazy recomputation
  - Maintains running OR of window contents
  - Only recomputes full window when leaving element had contributing bits
    (line 129: `leaving != 0 && window_or & leaving != 0`)
  - O(height) for sparse masks since recomputation is rare when most rows are zero
  - Falls back to O(height * radius) worst case for dense masks

### Comparison with Industry Standards

**Separable approach**: Standard optimization, described in van Herk/Gil-Werman
algorithm literature. Reduces 2D O(r^2) to two 1D O(r) passes. This
implementation is further optimized for the bit-packed format.

**Bit-level operations**: The horizontal pass uses word-level bit shifts for
dilation, processing 64 pixels simultaneously. The `dilate_word_fast` function
(mod.rs line 149) shifts the current word left and right by each offset in
1..radius, ORing results. Cross-word contributions are handled by shifting
adjacent words in the opposite direction (lines 161-177).

**Sliding window OR**: The vertical pass avoids the naive approach of
re-computing the OR window from scratch at each row. Instead it uses
incremental updates, only falling back to full recomputation when a leaving
element may have contributed bits to the running OR (line 129). This is
especially efficient for the sparse masks typical in star detection.

**No SIMD**: Unlike the threshold mask module, dilation operates on packed
u64 words directly, which are already efficient since each bitwise OR
processes 64 pixels. SIMD would provide minimal benefit here since the
bottleneck is memory bandwidth, not compute.

### Radius Usage

| Context | Radius | Config field | Purpose |
|---------|--------|-------------|---------|
| Detection | 1 | hardcoded in detect.rs:115 | Connect fragmented detections |
| Background refinement | 3 (default) | `bg_mask_dilation` (config.rs:196) | Mask object wings |
| Precise ground preset | 5 | `bg_mask_dilation` (config.rs:518) | Conservative masking |

The detection dilation radius of 1 is conservative -- it only bridges 1-pixel
gaps between above-threshold pixels. This is sufficient for well-sampled PSFs
(FWHM > 2px) where the core pixels are already connected. For undersampled
PSFs, 8-connectivity (without dilation) may be more appropriate.

### Tests

Comprehensive test suite (829 lines) covering:

- Basic: empty, single pixel, radius 0/1/2, corners, edges (lines 13-142)
- Merging: nearby pixels joined by dilation (lines 124-142)
- Large radius: radius larger than image (lines 148-172)
- Word boundaries: widths 64, 65, 128, 200 -- critical for bit operations (lines 264-400)
- Large radius (>63): tests the slow path (lines 367-400)
- 2D patterns: vertical + horizontal expansion, cross-boundary (lines 403-562)
- Sliding window: dense columns, same-word, sparse-then-dense transitions (lines 565-684)
- Edge rows: first/last row only, radius equals height (lines 687-756)
- Naive comparison: full naive implementation verified against optimized (lines 786-828)

---

## Pipeline Integration

The three modules are orchestrated in `detector/stages/detect.rs`:

```
detect() (detect.rs line 50):
  1. Optional matched filter (lines 63-89)
  2. create_threshold_mask / create_threshold_mask_filtered (lines 95-107)
  3. count pixels above threshold (line 110)
  4. dilate_mask(&mask, 1, &mut dilated) (line 115)
  5. LabelMap::from_pool (line 120)
  6. extract_and_filter_candidates (line 125)
```

Buffer management uses `BufferPool` for zero-allocation reuse across
detection calls. BitBuffer2 masks and Buffer2<u32> label maps are acquired
from and released back to the pool.

For background refinement (background/mod.rs line 119-143), the same
threshold mask + dilation sequence is used but with a different sigma
threshold and larger dilation radius to create conservative object masks.

## Issues Found by Research

### ~~P1: Radius-1 Dilation Before Labeling~~ FIXED
- Dilation was removed from detect.rs. No dilation is applied before CCL.

### ~~P1: Default 4-Connectivity Is Non-Standard~~ FIXED
- Default changed to `Connectivity::Eight` (config.rs:273), matching SExtractor,
  photutils, and SEP.

### P1 (Safety): Unsafe Mutable Aliasing in mask_dilation
- **Location**: mask_dilation/mod.rs lines 40-42 and 76-80
- Both horizontal and vertical dilation passes create `*mut u64` from
  `output.words().as_ptr() as *mut u64`, where `output.words()` returns `&[u64]`.
- This creates a mutable raw pointer from a shared reference, which is undefined
  behavior under Rust aliasing rules (violates `&T` immutability guarantee).
- The SAFETY comments claim disjoint per-thread access, which addresses data races
  but not the aliasing violation.
- **Fix**: Get `*mut u64` before entering the parallel region via
  `output.words_mut().as_mut_ptr()`, or restructure to use `&mut` slicing.

### P2: AtomicUnionFind Capacity Overflow Silently Ignored
- **Location**: labeling/mod.rs line 731-735
- `make_set()` uses `fetch_add(1)` and checks `if (label as usize) <= self.parent.len()`.
  If capacity is exceeded, the label is returned without being stored in the parent array.
- Subsequent `find()` calls on this label would access out-of-bounds memory or return
  garbage. Pre-allocation at 5% of pixel count (line 408) is generous but not guaranteed.
- **Fix**: Either panic on overflow (assert), or grow the parent array.

### P3: No Path Compression in AtomicUnionFind::find
- **Location**: labeling/mod.rs lines 739-752
- The atomic `find()` chases parent pointers without path compression.
- Sequential `UnionFind` does full path compression (lines 628-663).
- For typical astronomical masks (few large components), this is not a bottleneck.
- Path compression with atomics requires CAS loops and is complex to implement correctly.

### P3: Missing SExtractor Cleaning Pass
- SExtractor applies a cleaning pass that removes spurious detections near bright star
  wings (CLEAN parameter). This implementation has no equivalent.
- The quality filter stage partially compensates, but dedicated cleaning would help
  in crowded fields with very bright stars.
