# lumos/src/common - Implementation Notes (AI)

## Module Overview

Thin re-export layer from the workspace-level `common` crate (`/common/src/`). The `lumos/src/common/mod.rs` file is 4 lines and re-exports only `Buffer2` and `BitBuffer2`. Most lumos code accesses additional `common` crate items (`cpu_features`, `cfg_x86_64`, `cfg_aarch64`, `test_utils`, `parallel`, `SharedFn`) directly via `use common::*` rather than through this re-export module.

**Declared as:** `pub(crate) mod common;` in `lumos/src/lib.rs`

## Architecture

### What lumos re-exports (via `crate::common`)

| Type | Source | Usage |
|------|--------|-------|
| `Buffer2<T>` | `common::buffer2` | ~60 files, core 2D pixel/data buffer |
| `BitBuffer2` | `common::bit_buffer2` | ~24 files, bit-packed boolean masks |

### What lumos uses directly from `common` crate (not re-exported)

| Item | Usage Pattern | Approx. Usage |
|------|---------------|---------------|
| `common::cpu_features::*` | Runtime SIMD dispatch | ~56 call sites across 20 files |
| `common::{cfg_x86_64, cfg_aarch64}` | Conditional compilation macros | Convolution, median filter |
| `common::test_utils::test_output_path` | Test output file paths | ~10 test files |
| `common::parallel::try_par_map_limited` | Concurrency-limited parallel map | Stacking cache |
| `common::SharedFn` | Shared callback wrapper | Stacking progress |

## Implementation Details

### Buffer2<T> (`common/src/buffer2.rs`)

Generic 2D buffer backed by `Vec<T>`. Row-major layout with `(x, y)` indexing where `index = y * width + x`.

- Constructor asserts `pixels.len() == width * height`
- `Deref<Target=[T]>` allows using as a flat slice
- `Index<(usize, usize)>`, `Index<usize>`, `Index<Range<usize>>` with both shared and mutable variants
- `IntoIterator` for owned, ref, and mut-ref
- `From<Buffer2<T>>` into `Vec<T>`
- `copy_from` asserts dimension match
- `new_default` requires `T: Default + Clone`; `new_filled` requires `T: Clone`
- `get`/`get_mut` use `debug_assert` for bounds (release mode: unchecked for performance)
- No memory alignment guarantees -- uses standard `Vec<T>` allocator

### BitBuffer2 (`common/src/bit_buffer2.rs`)

Bit-packed 2D boolean buffer using `u64` words. 8x memory reduction vs `Vec<bool>`.

- **Row alignment:** 128 bits (16 bytes, 2 words) for SIMD-friendly row starts
- **Memory alignment:** 16-byte aligned via `AVec<u64, ConstAlign<16>>` (from `aligned-vec` crate)
- **Stride:** `width.div_ceil(128) * 128` bits per row (includes padding)
- **Index trait quirk:** Returns `&true` or `&false` static references since bit references are impossible
- `count_ones` correctly masks padding bits in partial words
- `from_slice` packs bools into words respecting stride
- No `IndexMut` (documented limitation; use `set`/`set_xy` instead)
- `swap` does `std::mem::swap` on word storage (O(1) pointer swap for `AVec`)

### CPU Feature Detection (`common/src/cpu_features.rs`)

Cached SIMD feature detection using `OnceLock<X86Features>`.

**Architecture:**
- `X86Features` struct holds 6 boolean flags: `sse2`, `sse3`, `ssse3`, `sse4_1`, `avx2`, `fma`
- `get()` calls `OnceLock::get_or_init` which runs `is_x86_feature_detected!` once per feature on first call
- Non-x86_64 stub returns all-false
- Convenience functions: `has_sse2()`, `has_sse3()`, `has_ssse3()`, `has_sse4_1()`, `has_avx2()`, `has_avx2_fma()`

**Dispatch pattern used in lumos:**
```rust
// x86_64: runtime dispatch via cpu_features
#[cfg(target_arch = "x86_64")]
if cpu_features::has_avx2_fma() {
    return unsafe { simd_avx2::function(args) };
}
#[cfg(target_arch = "x86_64")]
if cpu_features::has_sse4_1() {
    return unsafe { sse::function(args) };
}

// aarch64: compile-time dispatch (NEON is baseline-guaranteed)
#[cfg(target_arch = "aarch64")]
return unsafe { neon::function(args) };

// scalar fallback
scalar::function(args)
```

**NEON note:** There is no `has_neon()` function. aarch64 NEON is mandatory (always available), so lumos dispatches via `#[cfg(target_arch = "aarch64")]` at compile time, which is correct.

### Architecture-Gating Macros (`common/src/macros.rs`)

- `cfg_x86_64! { ... }` -- shorthand for `#[cfg(target_arch = "x86_64")]`
- `cfg_aarch64! { ... }` -- shorthand for `#[cfg(target_arch = "aarch64")]`
- `id_type!($name)` -- generates strongly-typed UUID wrapper (not used in lumos)

### Concurrency Utilities (`common/src/parallel.rs`)

- `par_map_limited` / `try_par_map_limited`: Process items in parallel via rayon but cap inflight items to `max_concurrent` (chunked `par_iter`)
- Used by lumos stacking to limit memory pressure

### FNV-1a Hasher (`common/src/fnv.rs`)

Deterministic FNV-1a 64-bit hasher. Offset basis `0xcbf29ce484222325`, prime `0x100000001b3`. Algorithm: XOR byte into hash, then multiply by prime (correct FNV-1a order; FNV-1 multiplies then XORs).

### Test Utilities (`common/src/test_utils.rs`)

- `test_output_path(name)`: Returns `{workspace_root}/test_output/{name}`, creating directories as needed
- `ensure_test_output_dir()`: Idempotent via `OnceLock`

## Current State (Verified Correct)

1. **CPU feature detection is sound.** The `OnceLock` caching is correct and provides a measurable benefit over raw `is_x86_feature_detected!`. While `is_x86_feature_detected!` already caches internally via atomics, each invocation performs an atomic load. The `OnceLock` approach bundles all 6 feature checks into a single struct read after initialization, avoiding repeated atomic loads when checking multiple features in the dispatch chain (e.g., AVX2+FMA check then SSE4.1 fallback). This matches the approach recommended by the Rust internals discussion on "Better codegen for CPU feature detection" which notes that chained `&&` feature checks produce poor codegen with repeated atomic loads.

2. **BitBuffer2 stride/padding is correct.** The `count_ones` and `iter` methods properly exclude padding bits. The `stride_padding_isolation` test confirms corrupted padding bits do not affect results. 128-bit row alignment ensures SIMD loads never straddle cache lines within a row.

3. **Buffer2 bounds checking is appropriate.** `debug_assert` in `get`/`get_mut` catches bugs in debug builds while providing zero overhead in release. The constructor `assert_eq` for size mismatch is always active, which is correct since it is a logic error.

4. **Non-x86_64 stub is correct.** Returning all-false from `cpu_features::get()` on non-x86 platforms ensures the SIMD code paths are never taken, falling through to scalar or NEON (which uses compile-time gating instead).

5. **FNV-1a implementation is correct.** The offset basis, prime, and algorithm order (XOR then multiply) all match the IETF specification (draft-eastlake-fnv) and reference test vectors (e.g., `fnv1a64("") = 0xcbf29ce484222325`).

6. **SIMD alignment strategy is correct.** `Buffer2` uses unaligned `Vec<T>` memory, and all SIMD code consistently uses `_mm_loadu_ps`/`_mm256_storeu_ps` (unaligned intrinsics) -- verified across 133 unaligned calls vs 0 aligned calls. This avoids alignment-related segfaults. `BitBuffer2` uses 16-byte aligned `AVec` which is sufficient since its consumers only use scalar word operations, not SIMD loads on the bit storage.

7. **`par_map_limited` chunking is correct.** Results preserve input order because `par_iter().map().collect()` within each chunk preserves order, and chunks are processed sequentially via the outer `for` loop.

## Issues Found

### I1: `FloatExt::approximately_eq` uses only absolute tolerance

The `approximately_eq` trait uses a fixed absolute tolerance (`EPSILON = 1e-6`) for both `f32` and `f64`. Per float comparison literature (see `float_eq` crate documentation), absolute-only comparison breaks down for large magnitudes (values like 1e6 would need relative tolerance) and is overly permissive near zero. Industry best practice is to combine absolute and relative tolerance, or use ULPs (Units in Last Place).

**Current impact:** Low for this project. Used in only 4 call sites outside tests: 2 in `worker_events_funclib.rs` (comparing against 0.0, where absolute tolerance is correct) and 2 in `execution_graph.rs` tests (comparing small integer values). Lumos SIMD code uses its own tolerance helpers in `centroid/test_utils.rs` and does not use this trait.

**Risk:** If ever used for values far from zero (e.g., pixel coordinates in thousands, or accumulated flux values), it will silently produce wrong results.

### I2: `Buffer2` lacks `row(y)` and `rows()` methods

Manual `y * width..(y + 1) * width` row-slicing appears in 251 occurrences across 55 files. This is both an ergonomic burden and a source of potential off-by-one errors. Standard image processing libraries (`image` crate's `ImageBuffer`, `ndarray`'s `.row()`) provide row accessors.

### I3: `Buffer2` has no alignment guarantees for SIMD

`Buffer2` uses standard `Vec<T>` which has no alignment beyond `T`'s natural alignment (4 bytes for `f32`). While the codebase correctly uses unaligned SIMD loads everywhere, 32-byte aligned memory enables `_mm256_load_ps` (aligned loads) which can be faster when data is naturally aligned to cache line boundaries. The `aligned-vec` crate is already a dependency (used by `BitBuffer2`), so adding an optional aligned variant would be straightforward.

**Current impact:** Negligible. Modern x86 CPUs (since Sandy Bridge, 2011) show minimal performance difference between aligned and unaligned loads when data happens to be aligned. The unaligned intrinsics are the correct default choice. This is a non-issue in practice.

### I4: `BitBuffer2::iter` has per-element overhead

The iterator calls `get_xy` per bit, which performs division and modulo to convert linear index to (x, y) and then to word/bit indices. A word-at-a-time iterator could eliminate this overhead entirely.

**Current impact:** None. Hot paths (threshold masking, mask dilation, labeling) operate on `words()` directly. The iterator is only used for `Into<Vec<bool>>` conversions and test assertions.

### I5: `BitBuffer2::count_ones` iterates row-by-row

The current implementation loops over each row and handles partial words per row. For cases where width is a multiple of 64, this adds unnecessary overhead from the row-loop structure. A flat `words.iter().map(|w| w.count_ones()).sum()` with a single final-row mask correction would be simpler and likely faster.

**Current impact:** Low. `count_ones` is not on any hot path (used in tests and diagnostic output).

### I6: No `is_aarch64_feature_detected!` for SVE/SVE2

The cpu_features module has no aarch64 feature detection at all. NEON is correctly handled via compile-time `#[cfg]` since it is mandatory on aarch64. However, if SVE/SVE2 support is ever added (Rust is actively working on SVE intrinsics per the 2025h1 project goals), runtime detection via `is_aarch64_feature_detected!("sve")` would be needed. No action required now.

## Missing Features

### M1: `Buffer2::row(y)` / `row_mut(y)` / `rows()` / `rows_mut()`

Row accessor methods are the single most impactful missing feature. Would eliminate 251 manual row-indexing calculations. Standard in `image::ImageBuffer` (via `rows()`), `ndarray` (via `.row()`), and virtually all 2D image containers.

```rust
pub fn row(&self, y: usize) -> &[T] {
    let start = y * self.width;
    &self.pixels[start..start + self.width]
}
```

### Apple laptop: `Buffer2::as_mut_ptr()`

There are 67 call sites using `.as_mut_ptr()` via `Deref` to `&mut [T]`, typically through `.pixels_mut().as_mut_ptr()`. A direct `as_mut_ptr()` method (symmetric with the existing `as_ptr()`) would be a minor ergonomic improvement.

### M3: `BitBuffer2` bulk word iterator

An iterator that yields `(row_index, &[u64])` per row (or word-at-a-time with position info) would make it easier to write correct bulk operations without manually computing `words_per_row` offsets. Lower priority since existing hot-path code works directly with `words()` slices.

### M4: `Buffer2::sub_image` / windowed view

A non-owning view into a rectangular sub-region of a `Buffer2`. Used in centroid fitting (stamp extraction), convolution (border handling), and background estimation. Currently each consumer manually computes offsets. The `image` crate provides `.sub_image()` / `GenericImageView::view()` for this.

**Complexity note:** Would require a stride concept (the parent buffer's width), making it a separate type (e.g., `BufferView<'a, T>`). May not be worth the API complexity given current usage patterns.

## Recommendations

1. **Add `Buffer2::row(y)` and `row_mut(y)`.** This is the highest-value improvement. Implementation is trivial (4 lines per method), eliminates a large class of potential indexing bugs, and matches standard image processing library APIs.

2. **Add `Buffer2::as_mut_ptr()`.** One-liner, symmetric with existing `as_ptr()`, removes the need for `.pixels_mut().as_mut_ptr()` in SIMD code.

3. **Leave `FloatExt::approximately_eq` as-is for now.** Its current users compare near-zero values where absolute tolerance is correct. If general-purpose float comparison is needed, use the `approx` or `float_eq` crate instead of expanding this trait. Document the limitation.

4. **Leave `cpu_features` as-is.** The `OnceLock` caching strategy is correct and efficient. Missing features (`sse4_2`, `avx`, `avx512*`, aarch64 SVE) should be added on demand when lumos code needs them.

5. **Leave `Buffer2` alignment as-is.** All SIMD code correctly uses unaligned intrinsics. Adding aligned allocation would add API complexity with negligible performance benefit on modern CPUs.

6. **Leave `BitBuffer2::iter` as-is.** The per-element overhead is not a problem since hot paths use `words()` directly. The iterator serves convenience use cases (conversion to `Vec<bool>`, test assertions) where performance is irrelevant.

7. **`FnvHasher` has no tests.** While the implementation is correct against the specification, adding a test with known reference vectors (e.g., `fnv1a64("foobar") == 0x85944171f73967e8`) would guard against regressions.
