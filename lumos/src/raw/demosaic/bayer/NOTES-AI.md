# Bayer Demosaic — RCD Implementation

## Current Status: IMPLEMENTED (Rayon parallel, optimized)

`demosaic_bayer()` calls `rcd::rcd_demosaic()` — a full rayon-parallel implementation of the
RCD (Ratio Corrected Demosaicing) algorithm v2.3 by Luis Sanz Rodriguez.

## Files

- `mod.rs`: `CfaPattern` enum, `BayerImage` struct, `demosaic_bayer()` entry point
- `rcd.rs`: RCD algorithm (5 steps, rayon row-parallel, ~650 lines)
- `tests.rs`: 20 tests (11 CFA pattern + 9 RCD correctness)

## Algorithm: RCD (Ratio Corrected Demosaicing)

Reference: https://github.com/LuisSR/RCD-Demosaicing

**Step 1: Fused V/H Direction Detection**
- Step 1a: Full V-HPF² buffer computed in parallel
- Step 1b: Fused H-HPF² (sliding 3-element window, no per-row allocation) + VH_Dir
- `VH_Dir = V_Stat / (V_Stat + H_Stat)` — 0=vertical, 1=horizontal
- HPF: `(cfa[-3w] - cfa[-w] - cfa[+w] + cfa[+3w]) - 3*(cfa[-2w] + cfa[+2w]) + 6*cfa`

**Step 2: Low-Pass Filter**
- At R/B positions (stride 2): `lpf = cfa + 0.5*(N+S+W+E) + 0.25*(NW+NE+SW+SE)`

**Step 3: Green Channel Interpolation**
- Cardinal gradients, ratio-corrected estimation, VH-weighted blend

**Step 4: R/B Channel Interpolation**
- 4.0-4.1: P/Q diagonal direction detection
- 4.2: R/B at opposing CFA positions via diagonal color differences
- 4.3: R/B at green positions via cardinal color differences + VH direction

**Step 5: Border Handling**
- Bilinear for 4-pixel border. Only iterates actual border pixels (top/bottom bands + left/right edges).

## Constants

- `EPS = 1e-5`, `EPSSQ = 1e-10`, `BORDER = 4`
- `intp(a, b, c) = b + a*(c - b)`

## Parallelism

All compute-heavy steps use rayon `par_chunks_mut` or `into_par_iter` by row.
11 parallel dispatches total (CFA copy fused 3→1, Step 4.2 fused 2→1).
`SendPtr` pattern for CFA copy, Steps 4.2, and 4.3 (write current row, read neighbor rows).
`Strides` struct bundles `rw, rh, w1, w2, w3` to reduce argument count.

## Memory Layout

Working buffers in raw coordinate space (full raw_width × raw_height):
- `scratch` (v_hpf → lpf → pq_dir): triple-reused buffer
- `vh_dir`: V/H direction map
- `rgb_r, rgb_g, rgb_b`: planar R/G/B output
- `p_hpf, q_hpf`: half-width temp buffers (Step 4.0 scoped)

Peak: ~6P f32 + 1P half (P = raw_width × raw_height). For 8736×5856: ~1.3 GB.

## Performance

### Synthetic benchmark (no I/O, 24 MP = 6000×4000):

| Version | Time | MP/s | Improvement |
|---------|------|------|-------------|
| Serial baseline | 548ms | 44 | — |
| Rayon parallel | 150ms | 160 | 3.6x parallel speedup |
| + Fused Step 1 + border opt | 120ms | 200 | -20% from parallel |
| + Buffer reuse + dispatch fusion | 111ms | 216 | -8% from above |

### Real file (Canon CR2, 8736×5856, 51 MP):

| Comparison | Time | Speedup |
|------------|------|---------|
| Our RCD | 954ms | — |
| vs libraw PPG | 1559ms | **1.6x** |
| vs libraw AHD | 2731ms | **2.9x** |
| vs libraw DHT | 5627ms | **5.9x** |

### Profiling breakdown (single-threaded, P-core %, current):

| Component | % |
|-----------|---|
| Memory zeroing (clear_page_erms) | 7% |
| f32 ops (abs/clamp) | 8% |
| Step 1b (H-HPF + VH_Dir) | 10% |
| Step 4.3 (R/B at green) | 6% |
| Step 2 (LPF) | 6% |
| Step 4.0 (PQ HPF) | 4% |
| intp() | 4% |
| CFA copy | 3% |
| Step 1a (V-HPF) | 3% |
| Unresolved (rcd.rs:0) | 16% |

### Key optimizations applied:
1. **Fused Step 1**: V-HPF full buffer + inline H-HPF sliding window (eliminates 1 full pass + per-row alloc)
2. **Border iteration**: Only touches actual border pixels instead of scanning entire image
3. **Buffer triple-reuse**: scratch buffer serves as v_hpf → lpf → pq_dir (saves 2 large allocations)
4. **Dispatch fusion**: CFA copy 3→1 dispatch, Step 4.2 R/B 2→1 dispatch (reduces rayon sync overhead)

## Quality

Compared against libraw (linear regression normalized to remove WB differences):
- vs AHD: avg MAE=0.000887, r>0.9998
- vs PPG: avg MAE=0.000876, r>0.9999
- vs DHT: avg MAE=0.000858, r>0.9998

## CfaPattern

Enum with 4 variants: `Rggb`, `Bggr`, `Grbg`, `Gbrg`.
Methods: `color_at(y,x)→{0,1,2}`, `red_in_row(y)`, `pattern_2x2()`,
`flip_vertical()`, `flip_horizontal()`, `from_bayerpat(str)`.

## Further Optimization Opportunities

1. **Tiling**: RawTherapee uses 194×194 tiles with 9-pixel overlap for better cache locality
2. **AVX2 SIMD**: stride-2 inner loops not auto-vectorized by LLVM, but manual SIMD requires gather (stride-2 + diagonal neighbors) which benchmarks showed is slower than scalar for this access pattern

### Evaluated and rejected:
- **Step 2+3 fusion**: LPF computed inline 5× per pixel (45 ops) vs 1 cached read. No improvement.
- **Manual SIMD (AVX2)**: Stride-2 column access + diagonal/vertical neighbors require gather — slower than scalar L1 loads.
- **Uninit allocations**: OS lazy page allocation means first-touch cost shifts mid-computation. Slightly worse than vec zeroing.

## Benchmarks

- `bench_rcd_demosaic_core`: Synthetic data, isolates demosaic performance
- `bench_bayer_rcd_demosaic`: Real file, compares timing vs libraw PPG/AHD/DHT
- `bench_bayer_rcd_quality_vs_libraw`: Quality metrics (MAE, PSNR, correlation)

Run: `cargo test -p lumos --release bench_rcd -- --ignored --nocapture`
Run: `cargo test -p lumos --release bench_bayer -- --ignored --nocapture`
