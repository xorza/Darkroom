# Bayer Demosaic — RCD Implementation

## Current Status: IMPLEMENTED (Rayon parallel)

`demosaic_bayer()` calls `rcd::rcd_demosaic()` — a full rayon-parallel implementation of the
RCD (Ratio Corrected Demosaicing) algorithm v2.3 by Luis Sanz Rodriguez.

## Files

- `mod.rs`: `CfaPattern` enum, `BayerImage` struct, `demosaic_bayer()` entry point
- `rcd.rs`: RCD algorithm (5 steps, rayon row-parallel, ~600 lines)
- `tests.rs`: 20 tests (11 CFA pattern + 9 RCD correctness)

## Algorithm: RCD (Ratio Corrected Demosaicing)

Reference: https://github.com/LuisSR/RCD-Demosaicing

**Step 1: V/H Direction Detection**
- 6th-order high-pass filter: `(cfa[-3w] - cfa[-w] - cfa[+w] + cfa[+3w]) - 3*(cfa[-2w] + cfa[+2w]) + 6*cfa`
- Squared HPF summed over 3 consecutive rows/cols for smoothing
- `VH_Dir = V_Stat / (V_Stat + H_Stat)` — 0=vertical, 1=horizontal

**Step 2: Low-Pass Filter**
- At R/B positions (stride 2): `lpf = cfa + 0.5*(N+S+W+E) + 0.25*(NW+NE+SW+SE)`
- Used for ratio correction in Step 3

**Step 3: Green Channel Interpolation**
- Cardinal gradients with 4 absolute differences each (reach ±4 pixels)
- Ratio-corrected estimation: `N_Est = cfa[N] * 2*lpf_center / (eps + lpf_center + lpf_N)`
- Weighted blend: `V_Est = (S_Grad*N_Est + N_Grad*S_Est) / (N_Grad + S_Grad)`
- VH discrimination refinement: use neighborhood average when closer to 0.5 than center

**Step 4: R/B Channel Interpolation**
- 4.0-4.1: P/Q diagonal HPF (same structure as V/H but on diagonals)
- 4.2: Missing color at R/B positions via diagonal color differences
- 4.3: R and B at green positions via cardinal color differences + VH direction

**Step 5: Border Handling**
- Bilinear interpolation for 4-pixel border (3x3 neighborhood, 5x5 fallback)

## Constants

- `EPS = 1e-5` (division guard)
- `EPSSQ = 1e-10` (HPF minimum)
- `BORDER = 4` (pixels on each side)
- `intp(a, b, c) = b + a*(c - b)` (linear interpolation)

## Parallelism

All compute-heavy steps use rayon `par_chunks_mut` or `into_par_iter` by row:
- CFA copy: parallel by channel and row
- Step 1.1 (V HPF): `par_chunks_mut(rw)` by row
- Step 1.2 (VH_Dir): `par_chunks_mut(rw)` with per-row H HPF buffer
- Step 2 (LPF): `par_chunks_mut(rw)` by row
- Step 3 (Green): `par_chunks_mut(rw)` by row
- Step 4.0-4.1 (PQ HPF/Dir): `par_chunks_mut` by row
- Step 4.2 (R/B at opposing): `SendPtr` + `into_par_iter` over row indices (two passes: R-rows, B-rows)
- Step 4.3 (R/B at green): `SendPtr` + `into_par_iter` over row indices
- Output extraction: `par_chunks_mut(width*3)` by row

`SendPtr` pattern (same as X-Trans): raw pointer wrapper implementing Send+Sync
for parallel write access to non-overlapping row regions.

`Strides` struct bundles `rw, rh, w1, w2, w3` to reduce function argument count.

## Memory Layout

Working buffers in raw coordinate space (full raw_width × raw_height):
- `vh_dir`: f32, V/H direction map
- `lpf`: f32, low-pass filter (freed after Step 3)
- `rgb_r, rgb_g, rgb_b`: f32 × 3, planar R/G/B output
- `pq_dir`: f32, P/Q diagonal direction map (Step 4 only)
- P/Q HPF: half-resolution buffers (every other pixel)
- V HPF: full buffer (freed before Step 1.2)

Peak: ~7P f32 (P = raw_width × raw_height). For 8736×5856: ~1.4 GB.

## Performance

Benchmark on Canon CR2 (8736×5856, 51.2 MP):
- RCD demosaic only: ~1037ms
- vs libraw PPG: 1.5x faster
- vs libraw AHD: 2.6x faster
- vs libraw DHT: 5.5x faster

Synthetic benchmark (no I/O):
- 1000×1000 (1 MP): 15ms → 86 MP/s
- 4000×3000 (12 MP): 88ms → 141 MP/s
- 6000×4000 (24 MP): 153ms → 160 MP/s

Parallel speedup vs serial baseline: ~3.6x (24 MP: 548ms → 153ms)

## Quality

Compared against libraw (linear regression normalized to remove WB differences):
- vs AHD: avg MAE=0.000887, r>0.9998
- vs PPG: avg MAE=0.000876, r>0.9999
- vs DHT: avg MAE=0.000858, r>0.9998

## CfaPattern

Enum with 4 variants: `Rggb`, `Bggr`, `Grbg`, `Gbrg`.
Methods: `color_at(y,x)→{0,1,2}`, `red_in_row(y)`, `pattern_2x2()`,
`flip_vertical()`, `flip_horizontal()`, `from_bayerpat(str)`.

## Optimization Opportunities

1. **AVX2 SIMD**: 8 f32 lanes for gradient/LPF computation
2. **Memory reduction**: Fuse steps or use tiling to reduce peak memory
3. **Fast reciprocal**: `_mm256_rcp_ps` for ratio correction division
4. **Tiling**: RawTherapee uses 194×194 tiles with 9-pixel overlap

## Benchmarks

- `bench_rcd_demosaic_core`: Synthetic data, isolates demosaic performance
- `bench_bayer_rcd_demosaic`: Real file, compares timing vs libraw PPG/AHD/DHT
- `bench_bayer_rcd_quality_vs_libraw`: Quality metrics (MAE, PSNR, correlation)

Run: `cargo test -p lumos --release bench_rcd -- --ignored --nocapture`
Run: `cargo test -p lumos --release bench_bayer -- --ignored --nocapture`
