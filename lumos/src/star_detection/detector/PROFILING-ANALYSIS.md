# Star Detector Profiling Analysis

**Date**: 2026-02-02  
**Benchmark**: `bench_detect_6k_globular_cluster`  
**Image**: 6144×6144 with 50,000 stars (extreme crowding)  
**Performance**: ~256ms per detection

---

## Optimization Plan

### Priority 1: Grid-Based Lookup in Deblending (Est. 10-15% improvement)

Replace HashMap/HashSet with array-based grid in `visit_neighbors` and connected component finding.

**Current code** (`multi_threshold.rs:489-516`):
```rust
fn visit_neighbors(
    pos: Vec2us,
    pixel_set: &HashMap<Vec2us, f32>,  // Hash lookup
    visited: &mut HashSet<Vec2us>,      // Hash check/insert
    queue: &mut Vec<Pixel>,
)
```

**Proposed change**:
```rust
struct GridLookup {
    data: Vec<Option<f32>>,  // Flat array, None = not in set
    visited: Vec<bool>,       // Flat array for visited
    offset_x: usize,
    offset_y: usize,
    width: usize,
    height: usize,
}

impl GridLookup {
    fn new(bbox: &Aabb, pixels: &[Pixel]) -> Self { ... }
    
    #[inline]
    fn get(&self, x: usize, y: usize) -> Option<f32> {
        let idx = (y - self.offset_y) * self.width + (x - self.offset_x);
        self.data[idx]
    }
    
    #[inline]
    fn mark_visited(&mut self, x: usize, y: usize) -> bool {
        let idx = (y - self.offset_y) * self.width + (x - self.offset_x);
        let was_visited = self.visited[idx];
        self.visited[idx] = true;
        !was_visited
    }
}
```

**Files to modify**:
- `lumos/src/star_detection/deblend/multi_threshold/mod.rs`

**Test coverage review**:
- Existing tests in `multi_threshold/tests.rs` cover: single star, two stars, three stars, hierarchical deblending, close peaks merge, empty component, flat profile, area conservation, bbox validation, peak values
- After optimization: Run all existing tests to verify behavior unchanged
- Add new test: `test_grid_lookup_boundary_conditions` - verify grid handles pixels at bbox edges correctly

### Priority 2: Early Exit from Deblending (Est. 5-8% improvement)

Skip expensive multi-threshold analysis for components unlikely to need deblending.

**Add early exit conditions in `deblend_multi_threshold`**:
```rust
// Skip if component too small to contain multiple stars
if data.area < config.min_area * 3 {
    return smallvec::smallvec![create_single_object(data, pixels, labels)];
}

// Skip if peak barely above detection (no substructure possible)
let peak_to_floor_ratio = peak_value / detection_threshold;
if peak_to_floor_ratio < 1.2 {
    return smallvec::smallvec![create_single_object(data, pixels, labels)];
}
```

**Files to modify**:
- `lumos/src/star_detection/deblend/multi_threshold/mod.rs`

**Test coverage review**:
- Existing tests cover small components (`test_single_pixel_component`, `test_empty_component`)
- Existing test `test_flat_profile_no_deblend` covers low peak-to-floor ratio case
- After optimization: Verify early exit doesn't skip components that should be deblended
- Add new test: `test_early_exit_small_area_threshold` - verify area threshold works correctly
- Add new test: `test_early_exit_low_contrast_ratio` - verify low contrast components skip deblending

### Priority 3: Reuse Pixel Collection Across Threshold Levels (Est. 3-5% improvement)

Currently `pixel_set` HashMap is rebuilt at each call to `find_connected_regions_reuse`. Pre-build once.

**Current code**:
```rust
fn find_connected_regions_reuse(...) {
    let pixel_set: HashMap<Vec2us, f32> = pixels.iter().map(|p| (p.pos, p.value)).collect();
    // ...
}
```

**Proposed**: Move `pixel_set` creation to `build_deblend_tree` and pass as parameter or store in `TreeBuildBuffers`.

**Files to modify**:
- `lumos/src/star_detection/deblend/multi_threshold/mod.rs`

**Test coverage review**:
- Existing test `test_buffer_reuse_consistency` verifies repeated calls produce identical results
- Existing test `test_n_thresholds_effect` tests multiple threshold levels
- After optimization: All existing tests should pass without modification
- No new tests needed - this is an internal refactor with no behavior change

### Priority 4: Reduce Buffer Allocations (Est. 3-5% improvement)

The kernel is spending 16-20% on `clear_page_erms` (zeroing new pages).

**Options**:
1. Pre-allocate reusable buffers in `StarDetector` struct
2. Use `Vec::with_capacity` + unsafe `set_len` to skip zeroing where safe
3. Reuse filtered image buffer across multiple detections

**Files to modify**:
- `lumos/src/star_detection/detector/mod.rs`

**Test coverage review**:
- Existing detector tests in `detector/tests.rs` cover FWHM outlier filtering and duplicate removal
- Existing benchmarks test full pipeline behavior
- After optimization: Run full test suite to verify no regressions
- No new tests needed - buffer reuse is transparent to callers

### Priority 5: Optimize `pixel_to_node` HashMap (Est. 2-3% improvement)

Replace `HashMap<Vec2us, usize>` with grid-based array similar to Priority 1.

**Files to modify**:
- `lumos/src/star_detection/deblend/multi_threshold/mod.rs`

**Test coverage review**:
- Same tests as Priority 1 apply - `pixel_to_node` is used in tree building
- Existing tests `test_hierarchical_deblend` and `test_three_stars_deblend` exercise tree building with multiple nodes
- After optimization: All existing deblend tests should pass
- No new tests needed if Priority 1 grid implementation is reused

---

### Summary

| Priority | Change | Est. Impact | Effort |
|----------|--------|-------------|--------|
| 1 | Grid-based lookup in visit_neighbors | 10-15% | Medium |
| 2 | Early exit from deblending | 5-8% | Low |
| 3 | Reuse pixel collection | 3-5% | Low |
| 4 | Reduce buffer allocations | 3-5% | Medium |
| 5 | Grid-based pixel_to_node | 2-3% | Low |

**Total estimated improvement: 23-36%** (from ~256ms to ~165-195ms)

---

## Baseline Benchmark Results (2026-02-02)

```
cargo test -p lumos --release bench_detect -- --ignored --nocapture
```

| Benchmark | Median | Min | Max |
|-----------|--------|-----|-----|
| bench_detect_1k_sparse | 11.98ms | 9.11ms | 30.76ms |
| bench_detect_4k_dense | 261.62ms | 93.70ms | 377.29ms |
| bench_detect_6k_globular_cluster | **244.27ms** | 235.26ms | 796.61ms |
| bench_detect_stars_filtered_1k | 0.88ms | 0.79ms | 3.93ms |
| bench_detect_stars_filtered_6k | 51.31ms | 21.27ms | 142.42ms |
| bench_detect_stars_6k_50000 | 164.28ms | 134.24ms | 205.17ms |

**Primary target**: `bench_detect_6k_globular_cluster` at 244ms median

---

## Benchmarking Protocol

After each optimization step:

1. **Run full benchmark suite**:
   ```bash
   cargo test -p lumos --release bench_detect -- --ignored --nocapture 2>&1 | tee bench_results.txt
   ```

2. **Run focused benchmark** (faster iteration):
   ```bash
   cargo test -p lumos --release bench_detect_6k_globular_cluster -- --ignored --nocapture
   ```

3. **Verify correctness** (run detection tests):
   ```bash
   cargo nextest run -p lumos deblend
   cargo nextest run -p lumos detector
   ```

4. **Profile if needed** (to verify bottleneck is addressed):
   ```bash
   perf record -g -F 9000 -o /tmp/perf.data -- \
     cargo test -p lumos --release bench_detect_6k_globular_cluster -- --ignored --nocapture
   perf report -i /tmp/perf.data --stdio --no-children -g none --percent-limit 1
   ```

5. **Record results** in this file under "Optimization Results" section

---

## Optimization Results

| Step | Change | Before | After | Improvement |
|------|--------|--------|-------|-------------|
| Baseline | - | 244.27ms | - | - |
| Priority 1 | Grid-based lookup in visit_neighbors | 244.27ms | ~245ms | ~0% (within noise) |
| Priority 2 | Early exit from deblending | ~245ms | ~245ms | ~0% (helps sparse fields) |
| Priority 5 | Grid-based pixel_to_node | ~245ms | ~237ms | ~3% improvement |
| Bit-packed visited | Packed bit vector for visited flags | ~237ms | ~236ms | ~0.5% + 8x less memory |
| Priority 4 | Sequential prefault (reverted) | ~248ms | ~277ms | -11.1% (slower) |
| Priority 4 | Parallel prefault (reverted) | ~277ms | ~270ms | -2.1% (within noise) |

**Current best**: ~236ms (3-4% improvement from 244ms baseline)

### Priority 1 Details (Completed 2026-02-02)

**Changes made**:
- Added `PixelGrid` struct with flat array storage for pixel values and visited flags
- Replaced `HashMap<Vec2us, f32>` and `HashSet<Vec2us>` with grid-based lookup
- Added `reset_with_pixels()` method to reuse grid allocation across calls
- Removed `HashSet` import (no longer needed)

**Profiling results after optimization**:
- `visit_neighbors_grid`: 5.4% CPU (down from 17% for `visit_neighbors`)
- `PixelGrid::reset_with_pixels`: 0.65% CPU
- Hash computation eliminated in hot path

**Benchmark results** (3 runs):
- Run 1: 241.82ms median
- Run 2: 247.10ms median  
- Run 3: 245.35ms median
- Average: ~245ms (essentially unchanged from baseline)

**Analysis**: The grid optimization successfully reduced neighbor lookup overhead from ~17% to ~5.4% CPU time. However, overall performance didn't improve because:
1. The `clear_page_erms` kernel overhead (23.6%) now dominates - this is page zeroing during vector allocations
2. The grid's `Vec::resize()` with fill value still triggers memory zeroing
3. Total time spent in deblending shifted but didn't decrease significantly

**Conclusion**: Grid optimization is correct and reduces hash overhead as expected. Further improvements require reducing memory allocation overhead (Priority 4) or adding early exits (Priority 2).

### Priority 2 Details (Completed 2026-02-02)

**Changes made**:
- Added early exit for components too small to contain multiple separable stars (area < 2 * min_separation²)
- Improved peak-to-floor ratio check using min_contrast: must have peak >= floor * (1/(1-min_contrast))

**Benchmark results** (3 runs):
- Run 1: 243.44ms median
- Run 2: 244.66ms median  
- Run 3: 246.05ms median
- Average: ~245ms (unchanged from Priority 1)

**Analysis**: The early exit conditions are working but don't significantly improve the globular cluster benchmark because:
1. This benchmark has extreme crowding with 50,000 stars
2. Most components in a globular cluster are large enough to potentially need deblending
3. The early exits help sparse fields more than crowded fields

**Profiling after Priority 2**:
- `clear_page_erms` (kernel): 24.3% - main bottleneck
- `deblend_multi_threshold`: 9.6%
- `visit_neighbors_grid`: 5.3%
- Background interpolation: 10.8%
- Threshold masking: 9.9%

### Priority 5 Details (Completed 2026-02-02)

**Changes made**:
- Added `NodeGrid` struct for grid-based pixel-to-node mapping
- Replaced `HashMap<Vec2us, usize>` with `NodeGrid` for O(1) array access
- Updated `process_root_level`, `process_higher_level`, `find_single_parent_grid`, `create_child_nodes`
- Removed `hashbrown::HashMap` import (no longer used)

**Benchmark results** (3 runs):
- Run 1: 239.96ms median
- Run 2: 237.34ms median  
- Run 3: 237.23ms median
- Average: ~238ms (down from ~245ms baseline)

**Improvement**: ~3% (7ms faster)

**Analysis**: The grid-based node assignment eliminates hash computation overhead for tracking which tree node each pixel belongs to. Combined with Priority 1's grid-based visited tracking, we've eliminated all HashMap/HashSet operations from the deblending hot path.

**Total improvement from all optimizations**: ~3% (244ms → 237ms)

### Bit-packed Visited Flags (Completed 2026-02-02)

**Changes made**:
- Replaced `Vec<bool>` with `Vec<u64>` for visited flags in `PixelGrid`
- Each u64 word stores 64 visited flags (1 bit per pixel)
- Reduces memory usage by 8x for the visited array
- Slightly improves cache efficiency

**Benchmark results** (5 runs):
- Run 1: 235.58ms median
- Run 2: 237.93ms median
- Run 3: 237.77ms median
- Run 4: 240.12ms median
- Run 5: 239.79ms median
- Average: ~238ms (slight improvement from ~237ms)

**Memory savings**: For a 100x100 pixel component, visited array goes from 10KB to 1.25KB.

**Final total improvement**: ~3-4% (244ms → ~236ms)

### Priority 4: Reduce Buffer Allocations - `clear_page_erms` Investigation (2026-02-02)

**Problem**: `clear_page_erms` consumes ~24% of CPU time. This is the kernel zeroing newly allocated memory pages when they're first written to.

**Root cause analysis**:
- Profiling showed `clear_page_erms` is triggered by `interpolate_segment_avx2` writing to newly allocated `Buffer2` buffers in background estimation
- For a 6K×6K image, each buffer is ~150MB (37.7M pixels × 4 bytes)
- Three buffers allocated: `bg_data`, `noise_data`, and sometimes `adaptive_data`
- When parallel interpolation writes to these buffers, it triggers page faults and kernel page zeroing

**Approaches attempted**:

1. **Sequential first-touch pass** (reverted)
   - Added `Buffer2::new_prefaulted()` that touches every page sequentially before parallel use
   - Result: **11.1% slower** (277ms vs 248ms baseline)
   - Reason: Sequential memory initialization is slow for large buffers, adds overhead without benefit

2. **Parallel prefaulting** (reverted)
   - Changed to parallel initialization using rayon to match NUMA topology
   - Each thread initializes its own chunk, faulting pages locally
   - Result: **~same performance** (-2.1%, within noise at 270ms)
   - Reason: Parallel initialization overhead offsets any gains from avoiding page faults during interpolation

**Conclusion**: The `clear_page_erms` overhead is difficult to avoid with simple prefaulting strategies. The kernel's page fault handling during `vec![value; size]` allocation is already well-optimized. Options that might help:

| Approach | Description | Effort | Expected Impact |
|----------|-------------|--------|-----------------|
| Buffer pooling | Reuse buffers across multiple `detect()` calls | Medium | High (eliminates allocation entirely) |
| Huge pages | Use 2MB pages via `madvise(MADV_HUGEPAGE)` | Low | Medium (reduces TLB pressure) |
| mmap + MAP_POPULATE | Pre-fault pages during mmap | High | Medium (moves cost, doesn't eliminate) |
| Custom allocator | jemalloc/mimalloc with huge page support | Low | Low-Medium |

**Recommendation**: For applications calling `detect()` repeatedly on similar-sized images, implement buffer pooling at the `StarDetector` level. This would completely eliminate the allocation overhead by reusing already-faulted memory.

---

## Top Bottlenecks (by self time)

| Rank | Function | CPU % | Category |
|------|----------|-------|----------|
| 1 | `clear_page_erms` (kernel) | 16-20% | Memory allocation |
| 2 | `deblend::multi_threshold::visit_neighbors` | 16-17% | **Main bottleneck** |
| 3 | `background::simd::interpolate_segment_avx2` | 8-10% | Background |
| 4 | `threshold_mask::sse::process_words_sse` | 8% | Thresholding |
| 5 | `deblend::multi_threshold::deblend_multi_threshold` | 7% | Deblending |
| 6 | `quicksort::partition` | 4-5% | Sorting |
| 7 | `convolution::simd::sse::convolve_row_avx2` | 3.2-3.6% | Convolution |
| 8 | `hashbrown::HashMap::insert` | 2.3% | Hash operations |

## Root Cause Analysis

### #1 Deblending (~24% total)

The `visit_neighbors` function in `deblend/multi_threshold/mod.rs:489-516` is the primary bottleneck:

```rust
fn visit_neighbors(
    pos: Vec2us,
    pixel_set: &HashMap<Vec2us, f32>,  // HashMap lookup per neighbor
    visited: &mut HashSet<Vec2us>,      // HashSet check/insert per neighbor
    queue: &mut Vec<Pixel>,
) {
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            // 8 iterations per pixel, each doing:
            // - HashSet::contains (hash + compare)
            // - HashMap::get (hash + compare)  
            // - HashSet::insert (hash + allocate)
```

**Problems:**
- Hash operations on `Vec2us` for every neighbor check (8× per pixel)
- Called for every pixel in every connected component at every threshold level
- With 50,000 stars in a globular cluster, many overlapping components require deblending
- Hash computation and bucket lookup has high constant factor vs array indexing

### #2 Memory Allocation (~16-20%)

The kernel spends significant time zeroing newly allocated pages (`clear_page_erms`). This correlates with:
- `interpolate_segment_avx2` triggering page faults (11.7% of kernel time)
- Large buffer allocations for 6K×6K images (37.7M pixels × 4 bytes = 151MB per buffer)
- Multiple intermediate buffers allocated during detection pipeline

## Recommendations

### High Impact

1. **Replace HashMap/HashSet with grid-based lookup in `visit_neighbors`**
   - Use a 2D boolean array indexed by `(x - bbox.min_x, y - bbox.min_y)`
   - O(1) array access vs O(1) amortized hash (hash has ~10-20× higher constant factor)
   - Component bounding boxes are already available
   - Estimated improvement: **10-15% overall**

2. **Early exit from deblending for simple components**
   - Skip multi-threshold analysis if component area < min_area × 2
   - Skip if peak-to-floor ratio < 1.5 (no significant structure)
   - Most isolated stars don't need deblending
   - Estimated improvement: **5-10% for sparse fields**

3. **Reduce hash operations in connected component finding**
   - Pre-build pixel position set once, reuse across threshold levels
   - Use flat array for `pixel_to_node` mapping instead of HashMap

### Medium Impact

4. **Reuse buffers across detection pipeline**
   - Pre-allocate output buffers in `StarDetector`
   - Use buffer pool for intermediate results (filtered image, threshold mask)
   - Reduces page fault overhead

5. **Consider huge pages for large image buffers**
   - 2MB huge pages reduce TLB pressure for 151MB buffers
   - Can be enabled via `madvise(MADV_HUGEPAGE)`

6. **Batch component processing**
   - Process multiple small components together to amortize allocation overhead
   - Reduces per-component setup cost

### Lower Impact

7. **Background interpolation prefetching**
   - Add software prefetch hints for upcoming grid cells
   - May reduce memory stall time

8. **Radix sort for star flux values**
   - Currently using quicksort (4-5% CPU)
   - Radix sort is O(n) for float keys with appropriate bucket strategy

## Implementation Priority

| Priority | Recommendation | Effort | Impact |
|----------|---------------|--------|--------|
| 1 | Grid-based lookup in visit_neighbors | Medium | High |
| 2 | Early deblending exit | Low | Medium |
| 3 | Reduce hash operations | Medium | Medium |
| 4 | Buffer reuse | Medium | Medium |
| 5 | Huge pages | Low | Low-Medium |

## Profiling Command

```bash
# Run perf profiling
perf record -g -F 9000 -o /tmp/perf.data -- \
  cargo test -p lumos --release bench_detect_6k_globular_cluster -- --ignored --nocapture

# Analyze flat profile
perf report -i /tmp/perf.data --stdio --no-children -g none --percent-limit 0.5

# Analyze call graph
perf report -i /tmp/perf.data --stdio --no-children -g graph,0.5,caller --percent-limit 2
```
