# Connected Component Labeling

This module implements connected component labeling (CCL) for binary masks using an RLE-based union-find algorithm optimized for sparse astronomical images.

## Algorithm Overview

The implementation uses a **two-pass RLE-based algorithm** with union-find:

1. **Run Extraction**: Convert each row of the binary mask into runs (contiguous foreground segments)
2. **Labeling & Merging**: Assign labels to runs and merge overlapping runs from adjacent rows using union-find
3. **Label Flattening**: Remap provisional labels to sequential final labels (1, 2, 3, ...)

### Why RLE-Based?

Traditional pixel-by-pixel CCL algorithms process every pixel, which is inefficient for sparse masks typical in star detection (< 5% foreground pixels). RLE-based approaches:

- Skip large background regions entirely
- Process entire runs instead of individual pixels
- Reduce memory bandwidth by working with compact run representations
- Scale with mask sparsity rather than image size

## Implementation Details

### Run-Length Encoding

Each row is encoded as a sequence of `Run` structs:
```rust
struct Run {
    start: u16,  // Starting x coordinate (inclusive)
    end: u16,    // Ending x coordinate (exclusive)
    label: u32,  // Provisional label
}
```

Run extraction uses **word-level bit scanning**:
- Process 64 bits at a time using the mask's internal `u64` word storage
- Fast-path for all-zero words (skip entirely) and all-one words (extend run)
- Bit-by-bit processing only for mixed words

### Union-Find Data Structure

The union-find (disjoint-set) structure tracks which runs belong to the same component. Two optimizations provide near-constant time operations:

1. **Union by rank**: Always attach the smaller tree under the root of the larger tree, preventing degenerate linear chains
2. **Path compression**: During `find()`, make all traversed nodes point directly to the root

With both optimizations, any sequence of m operations on n elements runs in O(m * α(n)) time, where α is the inverse Ackermann function (effectively constant, ≤ 4 for any practical n).

### Connectivity Modes

- **4-connectivity** (default): Runs must share at least one x coordinate to be connected
- **8-connectivity**: Runs can be diagonally adjacent (useful for undersampled PSFs)

The `Run` struct encapsulates connectivity-specific search window calculation:
```rust
impl Run {
    fn search_window(&self, connectivity: Connectivity) -> (u16, u16) {
        match connectivity {
            Connectivity::Four => (self.start, self.end),
            Connectivity::Eight => (self.start.saturating_sub(1), self.end + 1),
        }
    }
}

fn runs_connected(prev: &Run, curr: &Run, connectivity: Connectivity) -> bool {
    match connectivity {
        Connectivity::Four => prev.start < curr.end && prev.end > curr.start,
        Connectivity::Eight => prev.start < curr.end + 1 && prev.end + 1 > curr.start,
    }
}
```

### Parallel Processing

For large images (> 100k pixels), the algorithm switches to parallel mode:

1. **Strip division**: Split image into horizontal strips (64 rows minimum per strip)
2. **Parallel labeling**: Each strip labeled via `label_strip()` using shared `AtomicUnionFind`
3. **Boundary merging**: Merge labels at strip boundaries via `merge_strip_boundary()` with lock-free CAS
4. **Parallel output**: Write final labels to output buffer in parallel

The union-find is encapsulated in dedicated structs:
```rust
// Sequential processing (small images)
struct UnionFind {
    parent: Vec<u32>,
    next_label: u32,
}

impl UnionFind {
    fn make_set(&mut self) -> u32 { ... }
    fn find(&mut self, label: u32) -> u32 { ... }  // with path compression
    fn union(&mut self, a: u32, b: u32) { ... }
    fn flatten_labels(&mut self, labels: &mut [u32]) -> usize { ... }
}

// Parallel processing (large images)
struct AtomicUnionFind {
    parent: Vec<AtomicU32>,
    next_label: AtomicU32,
}

impl AtomicUnionFind {
    fn make_set(&self) -> u32 { ... }          // atomic increment
    fn find(&self, label: u32) -> u32 { ... }  // read-only traversal
    fn union(&self, a: u32, b: u32) { ... }    // CAS-based merging
    fn build_label_map(&self, total: usize) -> Vec<u32> { ... }
}
```

## Performance

Benchmarks on synthetic star fields (Apple M-series, release mode):

| Image Size | Stars  | Time (median) |
|------------|--------|---------------|
| 1024x1024  | 500    | ~400 µs       |
| 4096x4096  | 2000   | ~2 ms         |
| 4096x4096  | 50000  | ~9 ms         |

The RLE optimization provides ~50% speedup over pixel-based approaches for typical star detection masks.

## Comparison with Best Practices

### Implemented Optimizations

| Technique | Status | Notes |
|-----------|--------|-------|
| RLE representation | Yes | Compact encoding, fast extraction |
| Union by rank | Partial | Uses smaller-root-wins heuristic |
| Path compression | Yes | In sequential `UnionFind::find()` |
| Parallel strip processing | Yes | Lock-free `AtomicUnionFind` |
| Word-level bit scanning | Yes | 64-bit words with fast-paths |
| 8-connectivity | Yes | Via `Run::search_window()` and `runs_connected()` |
| Struct-based organization | Yes | Encapsulated `UnionFind` and `AtomicUnionFind` |

### Potential Improvements

1. **SIMD RLE extraction**: Use AVX2/NEON for parallel bit scanning (research shows ~5x speedup possible)
2. **Atomic path compression**: Currently read-only in parallel mode; adding compression may reduce tree depth
3. **Precomputed lookup tables**: Cache 16-bit binary patterns for faster run detection (as in recent MDPI paper)

## API

```rust
// Basic usage (4-connectivity)
let label_map = LabelMap::from_mask(&binary_mask);
let num_components = label_map.num_labels();

// With 8-connectivity for undersampled PSFs
let label_map = LabelMap::from_mask_with_connectivity(
    &binary_mask,
    Connectivity::Eight
);

// Access labels
let label_at_pixel = label_map[y * width + x];
let all_labels: &[u32] = label_map.labels();
```

## References

- [Union-Find Algorithm (GeeksforGeeks)](https://www.geeksforgeeks.org/dsa/union-by-rank-and-path-compression-in-union-find-algorithm/) - Path compression and union by rank
- [RLE-based CCL (GitHub)](https://github.com/ckhroulev/connected-components) - Standard two-scan RLE algorithm
- [SIMD RLE Algorithms (HAL Science)](https://hal.science/hal-02492824/document) - SIMD acceleration techniques
- [Novel CCL Algorithm (MDPI)](https://www.mdpi.com/1999-4893/18/6/344) - Recent optimizations with precomputed lookup tables

## Test Coverage

The module includes 45 tests covering:
- Basic shapes: single pixel, lines, L-shapes, U-shapes
- Edge cases: empty masks, zero dimensions, word boundaries
- Parallel processing: strip boundaries, large images, sparse/dense patterns
- RLE-specific: long runs, multiple runs per row, run merging
- 8-connectivity: diagonal connections, checkerboard patterns
