# lumos/src/common - Implementation Notes (AI)

## Module Overview

Thin re-export layer from the workspace-level `common` crate (`/common/src/`). The `lumos/src/common/mod.rs` file is 4 lines and re-exports only `Buffer2` and `BitBuffer2`. Most lumos code accesses additional `common` crate items (`cpu_features`, `cfg_x86_64`, `cfg_aarch64`, `test_utils`, `parallel`, `SharedFn`) directly via `use common::*` rather than through this re-export module.

**Declared as:** `pub(crate) mod common;` in `lumos/src/lib.rs`

### What lumos re-exports (via `crate::common`)

| Type | Source | Usage |
|------|--------|-------|
| `Buffer2<T>` | `common::buffer2` | ~60 files, core 2D pixel/data buffer |
| `BitBuffer2` | `common::bit_buffer2` | ~15 files, bit-packed boolean masks |

### What lumos uses directly from `common` crate (not re-exported)

| Item | Usage Pattern | Approx. Usage |
|------|---------------|---------------|
| `common::cpu_features::*` | Runtime SIMD dispatch | ~30 call sites |
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

### Test Utilities (`common/src/test_utils.rs`)

- `test_output_path(name)`: Returns `{workspace_root}/test_output/{name}`, creating directories as needed
- `ensure_test_output_dir()`: Idempotent via `OnceLock`

## Correctness Issues

No correctness bugs found. Specific verification:

1. **CPU feature detection is sound.** The `OnceLock` caching is correct and provides minor benefit over raw `is_x86_feature_detected!` (which already caches internally via atomics). The `OnceLock` approach bundles all feature checks into a single struct read after initialization, avoiding repeated atomic loads when checking multiple features. The `has_avx2_fma()` function correctly checks both AVX2 and FMA together (they are independent features; Intel CPUs since Haswell have both, but checking both is correct).

2. **BitBuffer2 stride/padding is correct.** The `count_ones` and `iter` methods properly exclude padding bits. The `stride_padding_isolation` test confirms corrupted padding bits do not affect results. 128-bit row alignment ensures SIMD loads never straddle cache lines within a row.

3. **Buffer2 bounds checking is appropriate.** `debug_assert` in `get`/`get_mut` catches bugs in debug builds while providing zero overhead in release. The constructor `assert_eq` for size mismatch is always active, which is correct since it is a logic error.

4. **Non-x86_64 stub is correct.** Returning all-false from `cpu_features::get()` on non-x86 platforms ensures the SIMD code paths are never taken, falling through to scalar or NEON (which uses compile-time gating instead).

## Recommendations

1. **Missing features in `X86Features`:** The struct does not include `sse4_2`, `avx`, or `avx512*` flags. Currently unused by lumos, but `avx` (as distinct from `avx2`) may be useful if SSE-width 256-bit float operations are ever needed. No action needed now; add on demand.

2. **The `lumos/src/common/mod.rs` re-export layer is minimal but inconsistent.** Lumos code uses `crate::common::Buffer2` (through the re-export) but `common::cpu_features` (directly from the crate). The re-export module exists to give lumos-internal code a `crate::common::Buffer2` path while keeping `Buffer2` and `BitBuffer2` as the only "core types" worth re-exporting. This is fine -- adding more re-exports would not improve clarity.

3. **`FloatExt::approximately_eq` for f64 uses f32 epsilon.** The `approximately_eq` implementation for `f64` casts `EPSILON` (1e-6 f32) to f64, which is effectively 1e-6 f64. This may be too loose or too tight depending on context. However, this trait is not used within lumos SIMD code (which uses its own tolerance helpers in `centroid/test_utils.rs`), so it is a non-issue for this crate.

4. **`BitBuffer2::iter` is element-by-element.** The iterator calls `get_xy` per bit, which involves a division and modulo per element. For bulk iteration, working with `words()` directly would be faster. This is not a performance issue in practice because hot paths (threshold mask, dilation) operate on words directly via SIMD, not through the iterator.

5. **No `Buffer2::row(y)` or `rows()` method.** Many consumers manually compute `y * width..(y + 1) * width` slices. A `row(y) -> &[T]` method would be a small ergonomic improvement. Low priority since `Deref<Target=[T]>` already allows arbitrary slicing.
