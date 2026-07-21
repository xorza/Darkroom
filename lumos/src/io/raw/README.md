# raw/ - RAW Camera File Loading and Demosaicing

## Architecture

```
raw/
├── mod.rs              # Internal RAW decoders, RAII guards, sensor dispatch
├── normalize.rs        # SIMD u16-to-f32 normalization (SSE2/SSE4.1/NEON)
├── tests.rs            # Unit tests for load_raw and normalization
├── benches.rs          # Benchmarks and quality comparison vs libraw
└── demosaic/
    ├── mod.rs           # Re-exports bayer and xtrans
    ├── bayer/
    │   ├── mod.rs       # CfaPattern, BayerImage types + demosaic_bayer() stub
    │   └── tests.rs     # CFA pattern and BayerImage validation tests
    └── xtrans/
        ├── mod.rs              # XTransPattern, XTransImage, process_xtrans()
        ├── hex_lookup.rs       # Hexagonal neighbor tables for Markesteijn
        ├── markesteijn.rs      # 1-pass orchestrator and buffer management
        └── markesteijn_steps.rs # 6-step algorithm: green, RGB, derivatives, blend
```

## Pipeline Flow

`PreviewImage::from_file(path)` dispatches RAW extensions to the private preview decoder, which:

1. Reads file into memory, opens via libraw FFI (`libraw_open_buffer`, `libraw_unpack`)
2. Extracts dimensions, margins, black/maximum levels
3. Calls `detect_sensor_type()` on libraw's `filters`/`colors` fields
4. Dispatches to one of four paths:

| Sensor Type | Normalization | Demosaic | Output |
|------------|---------------|----------|--------|
| Monochrome | `normalize_u16_to_f32_parallel()` + crop | None | 1-channel grayscale |
| Bayer | `normalize_u16_to_f32_parallel()` (SIMD) | RCD (Ratio Corrected Demosaicing) | 3-channel RGB |
| X-Trans | Per-channel normalization while reading the CFA | Markesteijn 1-pass (rayon) | 3-channel RGB |
| Unknown | libraw `dcraw_process` | libraw built-in | 3-channel RGB |

Every path keeps camera white balance at unity. RAW channel values therefore remain proportional
to sensor signal; color scaling belongs to a later explicit color-calibration or display step.
The camera-recorded multipliers are retained canonically as `[R, G1, B, G2]` in
`AstroImageMetadata` for optional downstream use, but are never applied during RAW decoding.

Both demosaic kernels emit unclipped linear RGB from one source-independent algorithm. Calibrated
`CfaImage` data therefore retains negative samples and interpolation overshoot. The direct
preview path applies its `[0,1]` contract once, after the finished image has been
assembled; range policy never changes interpolation or homogeneity selection.

RCD keeps its canonical low-pass ratio correction for every nonnegative or well-conditioned signed
neighborhood. When signed terms cancel enough to make the ratio ill-conditioned, a smoothstep
transition reaches the additive midpoint estimate `G₁ + (LPF₀−LPF₂)/8` at exact cancellation.
This keeps the estimator continuous and limits singular ratio amplification; exact canonical
behavior takes precedence over global pedestal equivariance away from cancellation.

## SIMD Strategy

### Normalization (`normalize.rs`)
- **SSE4.1** (preferred): `_mm_cvtepu16_epi32` for u16 zero-extension
- **SSE2** (fallback): `_mm_unpacklo_epi16` with zero register
- **NEON** (aarch64): `vmovl_u16` + `vcvtq_f32_u32`
- 4 elements per iteration, rayon parallel chunks of 16K
- Used by Monochrome, Bayer, and X-Trans paths

### X-Trans Demosaic (`xtrans/`)
- Precomputed hexagonal geometry and solitary-green phase for the 6×6 CFA
- Canonical three-stage red/blue reconstruction for solitary greens, opposite-color sites, and 2×2 green blocks
- Materialized `[red, blue]` candidates for each of the four green directions
- Summed area tables (SAT) for O(1) 5x5 window queries in blend step
- Uninitialized buffer allocation (`alloc_uninit_vec`) for all large intermediate arrays
- Buffer reuse: `gmin` is reinterpreted as `homo`; `gmax` becomes the derivative threshold
- Rayon parallelism via row-based and `(direction, row)` flattening
- `UnsafeSendPtr` wrapper for Edition 2024 closure captures

## Memory Optimization (X-Trans 6032x4028)

Peak is approximately 2.4 GiB for high-quality 4-direction interpolation. The 18·P-f32 arena is
about 1.63 GiB; final blending temporarily adds the planar output, homogeneity scores, and one SAT.

| Buffer | Size | Lifetime | Notes |
|--------|------|----------|-------|
| `green_dir` | 371 MiB | Steps 2-6 | Four directional green candidates |
| `red_blue_dir` | 742 MiB | Steps 3-6 | Four directional `[red, blue]` candidates |
| `drv` | 371 MiB | Steps 4-5 | Four directional YPbPr derivatives |
| `gmin` / `homo` | 93 MiB | Steps 1-2 / 5-6 | One arena region with disjoint lifetimes |
| `gmax` / `threshold` | 93 MiB | Steps 1-2 / 5 | One arena region with disjoint lifetimes |
| planar output | 278 MiB | Step 6 onward | Three f32 channels |
| homogeneity scores | 371 MiB | Step 6 | Four u32 scores per pixel |
| SAT | 93 MiB | Step 6 | One reused u32 summed-area table |

All large buffers use `alloc_uninit_vec` to skip kernel page zeroing (`clear_page_erms`).

## Test Coverage

Coverage includes RAW error paths and real-data fixtures, normalization and black levels, all Bayer
phases, X-Trans CFA geometry validation, hex lookup construction, signed demosaic invariants,
homogeneity and SAT behavior, and final direction blending. Markesteijn additionally has scalar
librtprocess golden samples for color edges, an impulse, a chromatic star profile, and a color
grating.

## Benchmarks

Run with `cargo test -p lumos --release <bench_name> -- --ignored --nocapture`:

- `raw_load` - End-to-end loading time (file read + unpack + normalize + demosaic)
- `bench_load_raw_libraw_demosaic` - libraw's built-in demosaic at different quality levels
- `bench_markesteijn_quality_vs_libraw` - Per-channel and false-color comparison after removing WB/scale differences

### Reference Numbers (X-Trans 6032x4028)
- Debug test build on the bundled 6032×4028 X-T2 RAF: ~4.2s demosaic / ~5.0s total
- Regression-normalized average MAE: ~0.00022 versus LibRaw 1-pass, ~0.00008 versus LibRaw 3-pass
- LibRaw 1-pass versus 3-pass baseline average MAE: ~0.00022
