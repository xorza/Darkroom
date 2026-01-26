# Imaginarium Conversion Optimization Plan

## Status: ✅ COMPLETE

All 5 phases have been implemented with SIMD optimizations:

| Phase | Conversion | Status | Measured Speedup |
|-------|-----------|--------|------------------|
| 1 | RGBA↔RGB U8 | ✅ Complete | 1.5-3.2x |
| 2 | Luminance U8 | ✅ Complete | 1.6-3.1x |
| 3 | U8↔F32 | ✅ Complete | 1.35-1.6x |
| 4 | U8↔U16 | ✅ Complete | 1.35-1.8x |
| 5 | F32 channels | ✅ Complete | 1.1-1.4x |

---

## Quick Benchmark Commands

Run only specific benchmarks to save time (full suite takes ~5+ minutes):

```bash
# Quick single conversion test (~5s)
cargo bench -p imaginarium --features bench --bench conversion -- "rgba_u8_to_rgb_u8/4096"

# Test specific phase
cargo bench -p imaginarium --features bench --bench conversion -- "rgba_u8_to_rgb\|rgb_u8_to_rgba"  # Phase 1
cargo bench -p imaginarium --features bench --bench conversion -- "to_l_u8\|l_u8_to"              # Phase 2
cargo bench -p imaginarium --features bench --bench conversion -- "u8.*f32\|f32.*u8"              # Phase 3
cargo bench -p imaginarium --features bench --bench conversion -- "u8.*u16\|u16.*u8"              # Phase 4
cargo bench -p imaginarium --features bench --bench conversion -- "f32.*rgb.*f32"                 # Phase 5

# By image size (4096 is most representative for SIMD gains)
cargo bench -p imaginarium --features bench --bench conversion -- "4096x4096"

# By category
cargo bench -p imaginarium --features bench --bench conversion -- "conversion_channels"
cargo bench -p imaginarium --features bench --bench conversion -- "conversion_bit_depth"
cargo bench -p imaginarium --features bench --bench conversion -- "conversion_luminance"

# Quick iteration with fewer samples (~20% faster)
cargo bench -p imaginarium --features bench --bench conversion -- "TARGET" -- --sample-size 50

# Compare against baseline
cargo bench -p imaginarium --features bench --bench conversion -- "TARGET" -- --save-baseline before
# ... make changes ...
cargo bench -p imaginarium --features bench --bench conversion -- "TARGET" -- --baseline before
```

---

## Current Performance (4096×4096 = 16.7M pixels)

| Conversion | Time | Throughput | Notes |
|------------|------|------------|-------|
| **U8 Channel (SIMD)** |
| RGBA→RGB U8 | 22.2ms | 756 Melem/s | SSSE3 shuffle |
| RGB→RGBA U8 | 21.3ms | 789 Melem/s | SSSE3 shuffle |
| **U8 Luminance (SIMD)** |
| RGBA→L U8 | 17.7ms | 947 Melem/s | PMADDUBSW |
| RGB→L U8 | 14.4ms | 1.16 Gelem/s | PMADDUBSW |
| L→RGBA U8 | 16.7ms | 1.00 Gelem/s | Broadcast |
| **Bit Depth (SIMD)** |
| RGBA U8→F32 | 57.4ms | 292 Melem/s | Unpack+convert |
| RGBA F32→U8 | 59.3ms | 283 Melem/s | Convert+pack |
| RGBA U8→U16 | 36.0ms | 466 Melem/s | (val<<8)\|val |
| RGBA U16→U8 | 36.1ms | 465 Melem/s | val>>8 |
| **F32 Channel (Rayon)** |
| RGBA→RGB F32 | 79.9ms | 210 Melem/s | Scalar+parallel |
| RGB→RGBA F32 | 78.5ms | 214 Melem/s | Scalar+parallel |

---

## Future Optimization Ideas

### High Priority (Good ROI)

1. **AVX2/AVX-512 paths** for x86_64 CPUs with wider SIMD
   - 256-bit AVX2: ~2x throughput over SSE
   - 512-bit AVX-512: ~4x throughput over SSE
   - Auto-detect with `is_x86_feature_detected!("avx2")`

2. **F32 Luminance SIMD** (currently scalar)
   - `RGBA_F32→L_F32`: 280 Melem/s → potential 500+ Melem/s
   - Use `_mm_dp_ps` (SSE4.1) or FMA for dot product

3. **L→RGBA F32 expansion** (currently scalar)
   - `L_F32→RGBA_F32`: 291 Melem/s → potential 500+ Melem/s
   - Simple broadcast operation

### Medium Priority

4. **RGB U8↔F32 SIMD paths** (currently only RGBA)
   - Would need 3-byte stride handling
   - Complex but doable with shuffle masks

5. **Tile-based processing** for cache optimization
   - Process 64×64 or 128×128 tiles
   - Better L2/L3 cache utilization on large images

6. **Buffer reuse API**
   - Avoid `Image::new_black()` allocation on each conversion
   - Add `convert_into(src, dst)` that reuses existing buffer

### Lower Priority

7. **LA (Luminance+Alpha) format support**
   - `RGBA→LA`, `LA→RGBA` conversions
   - Useful for grayscale with transparency

8. **In-place conversions** where possible
   - `RGBA_U8→RGB_U8` could work in-place (shrinking)
   - Reduces memory allocations

---

## Implementation Notes

### Architecture Detection

```rust
#[cfg(target_arch = "x86_64")]
if is_x86_feature_detected!("avx2") {
    // Use AVX2 path
} else if is_x86_feature_detected!("ssse3") {
    // Use SSSE3 path (current)
} else {
    // Fall back to scalar
}

#[cfg(target_arch = "aarch64")]
// NEON is always available on aarch64
```

### Key SIMD Techniques Used

| Technique | Intrinsic | Use Case |
|-----------|-----------|----------|
| Byte shuffle | `_mm_shuffle_epi8` | RGBA↔RGB reorder |
| Weighted sum | `_mm_maddubs_epi16` | Luminance calculation |
| Zero-extend | `_mm_unpacklo_epi8` | U8→U16, U8→U32 |
| Float convert | `_mm_cvtepi32_ps` | U32→F32 |
| Pack saturate | `_mm_packus_epi16` | U16→U8 |

### Memory Bandwidth Limits

At 4096×4096, operations are often memory-bound:
- DDR4-3200: ~25 GB/s theoretical, ~15-20 GB/s practical
- Current throughput: 5-10 GB/s effective (read+write)
- SIMD helps by reducing instruction count, not memory speed

---

## Files

- `src/common/conversion/conversion_simd.rs` - SIMD implementations
- `src/common/conversion/conversion.rs` - Scalar fallbacks
- `benches/conversion.rs` - Criterion benchmarks
- `bench-analysis.md` - Detailed analysis
- `bench-results.txt` - Raw benchmark output
