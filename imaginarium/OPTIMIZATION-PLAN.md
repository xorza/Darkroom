# Imaginarium Conversion Optimization Plan

## Current Status

### Completed SIMD Optimizations

| Conversion | Technique | Speedup |
|------------|-----------|---------|
| RGBA↔RGB U8 | SSSE3 PSHUFB shuffle | 1.5-3.2x |
| Luminance U8 | PMADDUBSW Rec.709 | 1.6-3.1x |
| U8↔F32 | SSE2 unpack + convert | 1.35-1.6x |
| U8↔U16 | Bit replication | 1.35-1.8x |
| F32 channels | Rayon parallel | 1.1-1.4x |
| F32 Luminance | SSE shuffle + FMA | 1.1-1.25x |
| L_F32↔RGBA_F32 | SSE broadcast | 1.1-1.2x |

### Current Throughput (4096×4096)

| Conversion | Throughput | Bottleneck |
|------------|------------|------------|
| RGB→L U8 | 1.16 Gelem/s | Compute |
| L→RGBA U8 | 1.00 Gelem/s | Memory |
| RGBA→RGB U8 | 757 Melem/s | Memory |
| U8→U16 | 466 Melem/s | Memory |
| U8→F32 | 292 Melem/s | Memory |
| F32→RGB F32 | 210 Melem/s | Memory |

---

## Phase 6: AVX2 Upgrade (High Impact)

**Goal**: 2x throughput on modern CPUs (89%+ support per Steam survey)

**Conversions to upgrade**:
1. RGBA↔RGB U8 - 256-bit shuffle with `_mm256_shuffle_epi8`
2. Luminance U8 - 256-bit `_mm256_maddubs_epi16`
3. U8↔F32 - 256-bit unpack/convert

**Implementation**:
```rust
#[cfg(target_arch = "x86_64")]
if is_x86_feature_detected!("avx2") {
    // Use AVX2 path
} else if is_x86_feature_detected!("ssse3") {
    // Fall back to SSSE3
}
```

**Expected improvement**: ~2x for cache-friendly sizes

**References**:
- [Simd Library](https://github.com/ermig1979/Simd) - C++ SIMD image processing
- [fast_image_resize](https://github.com/Cykooz/fast_image_resize) - Rust SIMD resizing

---

## Phase 7: Cache-Optimized Tiling (Medium Impact)

**Problem**: 4096×4096 images (67MB+) exceed L3 cache, causing memory bandwidth bottleneck.

**Solution**: Process in tiles that fit in L2 cache (256KB-1MB per core).

**Tile size calculation**:
- L2 cache: ~256KB per core
- RGBA_U8 tile: sqrt(256KB/4) ≈ 256×256 pixels
- Process tiles in Z-order (Morton curve) for better spatial locality

**Implementation approach**:
```rust
fn convert_tiled(from: &Image, to: &mut Image, tile_size: usize) {
    // Divide image into tiles
    // Process each tile with SIMD
    // Use rayon to parallelize across tiles
}
```

**Expected improvement**: 10-30% for large images

**References**:
- [Texture Tiling and Swizzling](https://fgiesen.wordpress.com/2011/01/17/texture-tiling-and-swizzling/)
- [Cache Tiling for Image Processing](https://www.researchgate.net/publication/225270282)

---

## Phase 8: Missing Format Paths

### High Priority

| From | To | Current | Priority |
|------|-----|---------|----------|
| RGB_U8 | RGB_F32 | Scalar | Add SIMD |
| RGB_F32 | RGB_U8 | Scalar | Add SIMD |
| LA_U8 | RGBA_U8 | Scalar | Add SIMD |
| RGBA_U8 | LA_U8 | Scalar | Add SIMD |

### Medium Priority

| From | To | Notes |
|------|-----|-------|
| RGB_U16 | RGB_F32 | HDR workflow |
| RGBA_U16 | RGBA_F32 | HDR workflow |
| L_U16 | L_F32 | HDR grayscale |

---

## Phase 9: API Improvements

### Buffer Reuse

Current issue: `Image::new_black()` allocates on every conversion.

```rust
// Current (allocates)
let converted = image.convert(ColorFormat::RGB_U8)?;

// Proposed (reuses buffer)
image.convert_into(&mut output_buffer, ColorFormat::RGB_U8)?;
```

### In-Place Conversions

For shrinking conversions (RGBA→RGB, RGBA→L):
```rust
// Could reuse input buffer when output is smaller
image.convert_in_place(ColorFormat::RGB_U8)?;
```

---

## Test Coverage Gaps

### Missing SIMD Correctness Tests

The current tests only verify scalar conversions. Need to add:

1. **Round-trip tests for all SIMD paths**:
   - RGBA_U8 → RGB_U8 → RGBA_U8
   - U8 → F32 → U8 (boundary values)
   - U8 → U16 → U8

2. **Edge case tests**:
   - Width not divisible by SIMD width (16 for SSE, 32 for AVX2)
   - Single-row images
   - Very small images (< SIMD width)

3. **Value correctness tests**:
   - Luminance weights (compare to known correct values)
   - F32 clamping (values > 1.0, < 0.0)
   - U16 scaling (0xFF → 0xFFFF, not 0xFF00)

### Suggested Test File

Create `imaginarium/src/common/conversion/simd_tests.rs`:

```rust
#[test]
fn test_rgba_to_rgb_u8_simd_correctness() {
    // Test various widths including non-SIMD-aligned
    for width in [1, 15, 16, 17, 31, 32, 33, 100, 256] {
        let src = create_test_rgba(width, 1);
        let dst = src.convert(ColorFormat::RGB_U8).unwrap();
        // Verify each pixel
    }
}

#[test]
fn test_luminance_weights() {
    // White pixel should give L=255
    // Pure red should give L ≈ 54 (0.2126 * 255)
    // Pure green should give L ≈ 183 (0.7152 * 255)
    // Pure blue should give L ≈ 18 (0.0722 * 255)
}

#[test]
fn test_u8_to_u16_boundary_values() {
    // 0 → 0
    // 255 → 65535 (not 65280)
}
```

---

## Benchmark Commands

```bash
# Quick single conversion
cargo bench -p imaginarium --features bench --bench conversion -- "rgba_u8_to_rgb_u8/4096"

# By category
cargo bench -p imaginarium --features bench --bench conversion -- "conversion_channels"
cargo bench -p imaginarium --features bench --bench conversion -- "conversion_luminance"
cargo bench -p imaginarium --features bench --bench conversion -- "conversion_bit_depth"

# Large images only (memory-bound)
cargo bench -p imaginarium --features bench --bench conversion -- "4096x4096"

# Compare before/after
cargo bench ... -- --save-baseline before
# make changes
cargo bench ... -- --baseline before
```

---

## Files

- `src/common/conversion/conversion_simd.rs` - SIMD implementations
- `src/common/conversion/conversion.rs` - Scalar fallbacks  
- `benches/conversion.rs` - Criterion benchmarks
- `bench-analysis.md` - Performance analysis
