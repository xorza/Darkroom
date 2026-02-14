# Bayer Demosaic

## Current Status

`demosaic_bayer()` in `mod.rs:189` contains `todo!("Bayer DCB demosaicing not yet
implemented")`. Any Bayer camera file (>95% of cameras: Canon, Nikon, Sony, etc.) panics.

The `CfaPattern` enum and `BayerImage` struct are complete with full validation and tests.
`color_at()`, `red_in_row()`, `pattern_2x2()`, `flip_vertical()`, `flip_horizontal()`,
`from_bayerpat()` are all implemented and tested for all 4 patterns (RGGB, BGGR, GRBG, GBRG).

## Algorithm Recommendation: RCD

RCD (Ratio Corrected Demosaicing) is the recommended algorithm for implementation.
Rationale:

1. **Star morphology**: RCD excels at round edges (stars), which is the primary concern
   for astrophotography. It reduces color overshooting artifacts that other algorithms
   (AMaZE, AHD) can produce around bright point sources.
2. **Speed**: RCD performs similarly to PPG (fast), much faster than AMaZE or AHD. This
   matters for batch processing of calibration frame stacks.
3. **Siril default**: Siril (the leading open-source astro processor) uses RCD as its
   default demosaicing algorithm for exactly these reasons.
4. **Manageable complexity**: ~300 lines of C in the reference implementation, well-structured
   sequential steps. Significantly simpler than AMaZE (~2000 lines).
5. **SIMD-friendly**: Row-parallel processing with regular 2x2 stride patterns.

Reference implementation: github.com/LuisSR/RCD-Demosaicing (rcd_demosaicing.c)

## RCD Algorithm Steps

The algorithm operates on normalized [0,1] CFA data:

**Step 1: V/H Direction Detection**
- Compute vertical and horizontal gradient statistics using 9-tap 1D kernels (indices -4 to +4)
- Direction = Stat_V / (Stat_V + Stat_H), producing a [0,1] map where 0=horizontal, 1=vertical
- Processes at 2-pixel stride (only R/B positions need direction info)

**Step 2: Low-Pass Filter (LPF)**
- Create reference signal from same-color CFA neighbors
- 5x5 checkerboard pattern at 2-pixel stride
- Used in the ratio correction formula

**Step 3: Green Channel Interpolation**
- At each R/B position, estimate green using ratio-corrected formula:
  `G_est = G_neighbor * (1 + (LPF_center - LPF_neighbor) / (LPF_center + LPF_neighbor))`
- Blend horizontal and vertical estimates using direction map from Step 1
- This is the key innovation: ratio correction in LPF domain instead of color difference domain

**Step 4: R/B Channel Interpolation**
- P/Q diagonal direction detection (similar to V/H but on diagonals)
- At B positions: interpolate R using green-guided color differences
- At R positions: interpolate B using green-guided color differences
- At G positions: interpolate both R and B using diagonal direction blending

**Step 5: Border Handling**
- 4-pixel margin processed with simpler interpolation (bilinear or nearest-neighbor)

## Memory Requirements

| Buffer | Size | Notes |
|--------|------|-------|
| `cfa` | W*H f32 | Input CFA (can alias normalized input) |
| `rgb` | W*H*3 f32 | Output RGB channels |
| `VH_Dir` | W*H f32 | Vertical/horizontal direction map |
| `PQ_Dir` | W*H f32 | Diagonal direction map |
| `lpf` | W*H f32 | Low-pass filtered reference (can be freed after Step 3) |
| **Total** | ~6P f32 | For 6000x4000: ~550 MB peak |

## SIMD Opportunities

The regular 2x2 Bayer structure is highly SIMD-friendly:

1. **Direction detection (9-tap)**: Process 4-8 pixels per SIMD lane with AVX2.
   The gradient computation is pure arithmetic (abs, add, compare).
2. **LPF computation**: Regular 5x5 checkerboard pattern maps naturally to SIMD.
3. **Green interpolation**: Ratio correction formula has division but can use
   Newton-Raphson reciprocal approximation (`_mm256_rcp_ps` + refinement).
4. **Color difference interpolation**: Average of 4 cardinal neighbors is trivially
   vectorizable.
5. **Row-parallel**: Every step processes complete rows independently -> rayon.

Expected performance: ~50-100ms for 6000x4000 Bayer image (based on X-Trans timing
scaled by algorithmic complexity and SIMD friendliness of 2x2 pattern).

## Implementation Plan

1. Port RCD reference C code to Rust (scalar first, ~300 lines)
2. Add rayon row-parallelism for each step
3. Add AVX2 SIMD for direction detection and green interpolation inner loops
4. Test against libraw AHD output (similar to existing Markesteijn quality benchmarks)
5. Benchmark against libraw's built-in demosaic at various quality levels

## Alternative: Interim Libraw Fallback

As an immediate fix before RCD implementation, `demosaic_bayer()` could delegate to
`demosaic_libraw_fallback()` with `user_qual=3` (AHD). This would:
- Unblock Bayer camera support immediately
- Be slower (~2-3x vs native RCD) but functional
- Require restructuring since `demosaic_libraw_fallback()` is on `UnpackedRaw` and needs
  the libraw instance, while `demosaic_bayer()` takes a `BayerImage` reference

## Algorithm Comparison

| Algorithm | Quality | Speed | Why Not Primary |
|-----------|---------|-------|-----------------|
| **RCD** | High | Fast | **Recommended** -- best star handling, Siril default |
| **AMaZE** | Highest | Slow | 4-6x slower, complex (~2000 LOC), color overshoots on stars |
| **DCB** | High | Medium | Good for no-AA-filter cameras, but RCD handles this case too |
| **VNG4** | Medium | Medium | Loses high-frequency detail. Good for sky but not stars |
| **AHD** | Medium | Slow | Old algorithm, generally inferior to RCD in all metrics |
| **LMMSE** | High | Medium | Best for high-ISO noise, but astro uses stacking for noise |
| **Bilinear** | Low | Fast | Only suitable for thumbnails/previews |
| libraw built-in | Variable | Variable | 0=linear, 1=VNG, 2=PPG, 3=AHD, 11=DHT, 12=AAHD |

**Future: Dual demosaic** (RCD + VNG4) would be ideal for astrophotography: RCD for
high-detail regions (stars, nebula edges), VNG4 for smooth sky background. Both
RawTherapee and darktable support this hybrid mode.
