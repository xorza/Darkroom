# Benchmark Plan for Imaginarium Image Format Conversion

## Overview

The goal is to benchmark scalar vs SIMD implementations for image format conversions without exposing private APIs. The pattern follows lumos: feature-gated `bench` modules placed alongside tested modules.

## Structure

```
imaginarium/
├── Cargo.toml                          # Add bench feature + criterion dep
├── benches/
│   └── conversion.rs                   # Entry point (minimal)
└── src/
    └── common/
        └── conversion/
            ├── mod.rs                  # Add: #[cfg(feature = "bench")] pub mod bench;
            ├── conversion_scalar.rs    # Existing scalar implementation
            ├── conversion_simd.rs      # Existing SIMD placeholder
            └── bench.rs                # NEW: Benchmark implementations
```

## Key Conversions to Benchmark

Since SIMD is not yet implemented (placeholder only), the benchmarks will initially measure **scalar performance** and establish baselines. When SIMD is added, the same benchmarks compare both.

### High-priority conversions (most common in image processing)

| Conversion | Description | Notes |
|------------|-------------|-------|
| RGBA_U8 → RGB_U8 | Drop alpha | Common for display/export |
| RGB_U8 → RGBA_U8 | Add opaque alpha | Common for GPU upload |
| RGBA_U8 → L_U8 | Color to grayscale | Luminance calculation |
| L_U8 → RGBA_U8 | Grayscale to color | Expand + add alpha |
| RGBA_U8 → RGBA_F32 | U8 to float | HDR/processing pipeline |
| RGBA_F32 → RGBA_U8 | Float to U8 | Export/display |
| RGB_U8 → RGB_F32 | U8 to float | Processing |
| RGB_F32 → RGB_U8 | Float to U8 | Export |

### Image sizes to benchmark

- 256×256 (small, cache-friendly)
- 1024×1024 (medium, typical)
- 4096×4096 (large, memory-bound)

## Implementation Details

### 1. Cargo.toml changes

```toml
[features]
bench = ["dep:criterion"]

[dependencies]
criterion = { workspace = true, optional = true }

[[bench]]
name = "conversion"
harness = false
required-features = ["bench"]
```

### 2. bench.rs pattern

- Uses `super::conversion_scalar::convert_pixels` directly (private access via `super::`)
- Creates synthetic test images with known patterns
- Benchmarks with `criterion::black_box` to prevent optimization
- Groups by conversion type, measures throughput in pixels/sec
- Platform-specific SIMD benchmarks with `#[cfg(target_arch = "...")]`

### 3. Module visibility

- `conversion_scalar` functions need `pub(super)` or direct bench access
- Bench module accesses siblings via `super::conversion_scalar::*`
- No public API changes needed

## Files to Create/Modify

1. **Modify** `imaginarium/Cargo.toml` - Add bench feature and criterion
2. **Create** `imaginarium/benches/conversion.rs` - Entry point
3. **Create** `imaginarium/src/common/conversion/bench.rs` - Benchmark implementations  
4. **Modify** `imaginarium/src/common/conversion/mod.rs` - Add bench module
5. **Modify** `imaginarium/src/lib.rs` - Export bench module when feature enabled

## Running Benchmarks

```bash
cargo bench -p imaginarium --features bench --bench conversion
```
