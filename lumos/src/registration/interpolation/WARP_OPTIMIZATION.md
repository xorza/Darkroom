# Warp Optimization Analysis

## Current Performance Profile

From perf profiling of `bench_warp` (Lanczos3, ~6000x4000 RGB image):

| Function | Self % | Description |
|----------|--------|-------------|
| `interpolate_lanczos` | 85.09% | Main hotspot - Lanczos kernel evaluation |

**Current time:** ~600ms per warp (3 channels)

## Bottleneck Analysis

The `interpolate_lanczos_impl` function dominates execution time due to:

1. **Per-pixel LUT lookups** - 12 LUT lookups per pixel (6 for X weights, 6 for Y weights for Lanczos3)
2. **Linear interpolation in LUT** - Each lookup involves float conversion, multiply, add
3. **36 multiply-accumulates** - 6x6 kernel window for each output pixel
4. **Non-SIMD scalar code** - All operations are scalar despite being highly parallelizable

## Optimization Strategies

### 1. Separable Filter (High Impact - Estimated 2-3x speedup)

**Current:** 2D convolution computes all 36 (6x6) samples per pixel  
**Proposed:** Two-pass 1D filtering (horizontal then vertical)

The Lanczos kernel is **separable** - it can be applied as two sequential 1D passes:

```
Output = Vertical_Pass(Horizontal_Pass(Input))
```

This reduces operations from O(n²) to O(2n) per pixel:
- Lanczos3: 36 → 12 multiplications per pixel
- Lanczos4: 64 → 16 multiplications per pixel

**Implementation:**
1. Allocate temporary buffer for horizontal pass results
2. Apply 1D Lanczos horizontally to all rows
3. Apply 1D Lanczos vertically to the intermediate buffer

**References:**
- [Intel AVX Lanczos Implementation](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html) - Uses separable approach with horizontal/vertical phases

### 2. SIMD Vectorization (High Impact - Estimated 4-8x speedup for inner loops)

**Current:** Scalar operations  
**Proposed:** AVX2/SSE4.1 for x86_64, NEON for aarch64

Key SIMD opportunities:
- **Weight computation:** Compute 4-8 LUT lookups in parallel
- **Multiply-accumulate:** Use `_mm256_fmadd_ps` for fused multiply-add
- **Horizontal sum:** Use `_mm256_hadd_ps` for coefficient sum

Example for weight computation (8 weights at once with AVX2):
```rust
// Load 8 dx values
let dx = _mm256_set_ps(dx7, dx6, dx5, dx4, dx3, dx2, dx1, dx0);
// Compute indices into LUT
let idx = _mm256_cvttps_epi32(_mm256_mul_ps(abs_dx, resolution));
// Gather from LUT
let weights = _mm256_i32gather_ps(lut_ptr, idx, 4);
```

**References:**
- [AVIR Library](https://github.com/avaneev/avir) - Fast SIMD Lanczos resizer achieving 245ms for 17.9 Mpixel → 2.5 Mpixel resize

### 3. Process Multiple Pixels Per Iteration (Medium Impact)

**Current:** One output pixel per inner loop iteration  
**Proposed:** Process 4-8 adjacent output pixels together

For adjacent output pixels:
- Transform coordinates are sequential (can use SIMD)
- Can batch LUT lookups
- Can reuse some source pixel reads (overlapping windows)

### 4. Optimize LUT Access (Medium Impact)

**Current:** Linear interpolation between LUT entries  
**Proposed:** Options:

a. **Direct LUT (no interpolation):** Increase LUT resolution to 4096+ entries, use direct indexing
   - Trades memory for fewer operations
   - LUT size for Lanczos3: 4096 * 3 * 4 bytes = 48KB (fits in L1 cache)

b. **Polynomial approximation:** Replace LUT with fast polynomial (sinc approximation)
   - Avoids memory access entirely
   - Can be vectorized easily

### 5. Cache-Friendly Access Pattern (Low-Medium Impact)

**Current:** Random access based on transform  
**Proposed:** 
- Process output in cache-line-sized tiles
- Prefetch source data for next tile
- Consider memory layout of intermediate buffer for separable filter

### 6. Reduce Branching (Low Impact)

**Current:** Bounds checking per sample  
**Proposed:**
- Pre-compute valid region, use fast path for interior pixels
- Use saturating arithmetic instead of conditional border handling

## Recommended Implementation Order

1. **Separable filter** - Largest algorithmic improvement, reduces operations by ~3x
2. **SIMD for 1D horizontal pass** - After separable, horizontal pass is perfectly vectorizable
3. **SIMD for 1D vertical pass** - Also vectorizable, benefits from sequential memory access
4. **Increase LUT resolution + direct lookup** - Removes linear interpolation overhead

## Expected Results

| Optimization | Estimated Speedup | Cumulative |
|-------------|------------------|------------|
| Baseline | 1.0x | 600ms |
| Separable filter | 2-3x | 200-300ms |
| SIMD horizontal | 2-4x | 75-150ms |
| SIMD vertical | 1.5-2x | 50-100ms |
| LUT optimization | 1.2-1.5x | 35-80ms |

**Target:** Sub-100ms for full RGB image warp

## Code References

- Bilinear SIMD implementation exists in `simd.rs` - can serve as template
- Intel IPP uses `_mm256_insertf128_ps` for packing, `_mm256_hadd_ps` for reduction
- Consider using `std::arch` directly or `packed_simd2` / `wide` crates

## Additional Resources

- [Intel IPP Lanczos AVX](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html)
- [AVIR Library (C++ SIMD Lanczos)](https://github.com/avaneev/avir)
- [Lanczos Resampling Explained](https://mazzo.li/posts/lanczos.html)
- [Efficient Lanczos on ARM NEON](https://www.researchgate.net/publication/322881948_Efficient_Projective_Transformation_and_Lanczos_Interpolation_on_ARM_Platform_using_SIMD_Instructions)
