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

### Priority 4: Reduce Buffer Allocations (Est. 3-5% improvement)

The kernel is spending 16-20% on `clear_page_erms` (zeroing new pages).

**Options**:
1. Pre-allocate reusable buffers in `StarDetector` struct
2. Use `Vec::with_capacity` + unsafe `set_len` to skip zeroing where safe
3. Reuse filtered image buffer across multiple detections

**Files to modify**:
- `lumos/src/star_detection/detector/mod.rs`

### Priority 5: Optimize `pixel_to_node` HashMap (Est. 2-3% improvement)

Replace `HashMap<Vec2us, usize>` with grid-based array similar to Priority 1.

**Files to modify**:
- `lumos/src/star_detection/deblend/multi_threshold/mod.rs`

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
| | | | | |

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
