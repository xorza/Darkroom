# lumos math performance

Work on branch `lumos-math-perf`. Every figure is criterion's median time against the baseline
`base` (commit `ef2d3cde8`), as a ratio new / base: under 1 is faster.

## Method

- Benches: `cargo bench -p lumos --features bench -- math/...` (one target, `benches/lumos.rs`).
- A/B: the baseline is built once in its own worktree (`../darkroom-perf-base`), and every run
  saves into one shared `CRITERION_HOME`, so a later build compares to `base` with no rebuild.
- Linux: AMD Ryzen 7 6800U, 8 cores / 16 threads, x86-64-v3 build. macOS: Apple Silicon (arm64),
  8 cores.
- An optimization keeps every result at least as accurate as before. A change that moves output
  bits says so in its row.

## Results

| Bench | Linux | macOS | Change |
|---|---|---|---|
| `noise/frame/mrs_estimate/1024` | 0.25 | 0.16 | Two-pass deviation without a division per value, in parallel shares added in a fixed order, and after an iteration's first pass one pass each, its deviations taken from the mean the iteration before found and corrected by their sum, which rounds as the second pass does (the frame statistics snapshot keeps its bits); coefficients stored as columns, so a pass reads only the columns it needs; a tile builds its first layer and significance as it first smooths, with no plane cleared or copied first; the fused à trous step below. Moves bits: the corrected two-pass sum (Chan, Golub & LeVeque) replaces Welford's update, and errs less. |
| `noise/frame/mrs_estimate/4096` | 0.30 | 0.16 | Same. |
| `noise/frame/mrs_estimate_split/1024` | 0.28 | 0.19 | Same; shares of 8192 values; the gain bins measured in parallel, each on its own; the samples tagged in parallel shares. Moves bits: each bin's gain sum is added share by share. |
| `noise/frame/mrs_estimate_split/4096` | 0.29 | 0.20 | Same. |
| `wavelet/frame/atrous_smooth/step_1/2048` | 0.51 | 0.52 | Both passes fused row by row: the five source rows convolved down the columns into one row of scratch, then along it, so the frame is read once and written once and no intermediate plane exists. Each tile, denoise and HDR hold one plane less. Moves bits: the columns are convolved before the rows, the same arithmetic in the other order. |
| `wavelet/frame/atrous_smooth/step_8/2048` | 0.55 | 0.57 | Same. |
| `lstsq/factor/2000x15` | 0.28 | 0.23 | A design taller than wide is reduced by a Householder QR and its SVD taken of the square `R`, as LAPACK's `gesvd` does: the same singular values and rank, both steps backward stable. Moves bits. |
| `lstsq/factor/500x10` | 0.37 | 0.31 | Same. |
| `lstsq/solve/2000x15` | 2.34 | 2.24 | A solve now applies the reflections to its right-hand side. Every caller factors once and solves once: factor and solve together are 0.32 of before at 2000×15. |
| `noise/frame/binned_measure/1000000` | 0.88 | | The bins measured in parallel. |
| `noise/gain_bin/4096` | 0.29 | 0.60 | The 7 bounds as a fixed array, the bin counted without a branch. Same bins. |
| `statistics/median_mut/16` | 0.52 | 0.74 | Selection on integer keys in `total_cmp`'s order, where `total_cmp` mapped both operands of every comparison. A slice with no sign bit set is its own keys, which one OR over its bits finds, so it is neither written nor mapped back; any other is mapped once each way. Same medians. |
| `statistics/median_mut/4096` | 0.78 | 0.91 | Same, and the even count's lower middle by a fold with `Ord::max`: `Iterator::max` compiled to a scalar loop, ten times the fold's time on arm64. |
| `statistics/median_mad/16` | 0.62 | 0.74 | Same, and the MAD selects the deviations' bits directly: a value with its sign bit clear is its own key. |
| `statistics/median_mad/4096` | 0.77 | 0.89 | Same. |
| `statistics/mad_with_scratch/4096` | 0.76 | 0.89 | Median of magnitudes. |
| `statistics/robust_sigma_f64/4096` | 0.78 | 0.95 | Both medians on keys. |
| `statistics/sigma_clipped/16` | 0.88 | 1.00 | The values are clipped as keys across every pass; the survivors close up without a branch per value; unmapped keys read back as the values' bits; a pass after the first searches for its median on one side only, as the one before left the keys partitioned at its middle and the survivors keep their order. `median_fast` (a comparison that is wrong for NaN) is gone: every median is the total-order one. |
| `statistics/sigma_clipped/4096` | 0.75 | 0.82 | Same. |
| `statistics/spread/16` | 0.72 | 0.95 | The MAD of a sorted window by a binary search over how many of its middle rank each side of the centre gives, the deviations two ascending runs: as many steps as the length has bits, where the merge took half the window. Same bits. |
| `statistics/spread/256` | 0.37 | 0.29 | Same. |
| `pixel_quadrature/sufficient_order/gaussian_0.5/1` | 0.48 | 0.42 | The orders bisected, as the bound falls with the order; each order's golden-section search ends at the first ρ whose bound is within the tolerance, which the least it would end on is at most. Same orders. The star fits run it twice a star. |
| `pixel_quadrature/sufficient_order/moffat_1.5_2.5/1` | 0.46 | 0.43 | Same. |
| `floor_i32/floor_i32/f32/4096` | 0.94 | 0.41 | The target's rounding instruction (`roundss` from SSE4.1, arm64's `frintm`) where it has one; truncation and one step down only on x86-64 without SSE4.1, where `floor` is a library call. Same values. |
| `floor_i32/floor_i32/f64/4096` | 0.94 | 0.67 | Same. |
| `pixel_quadrature/gauss_legendre/7` | 0.012 | | Every order's rule solved once, on first use, and copied out after, as the Lanczos tables are: the fits take one a star. Same nodes. |
| `pixel_quadrature/gauss_legendre/16` | 0.001 | | Same. |
| `statistics/frame/plane_median/radix/24000000` | 0.93 | 0.67 | `add_all` takes a run: eight keys at a time, and the second pass tests eight at once and counts only those in the middle bins. Normalization hands it every full mask word. |

The `statistics` rows are against a baseline measured again after the bench fix that batches in-place
inputs to stay in cache (`52e54d2c6`): before it, a batch of copies outgrew the cache and the
4096-value figures moved by up to 2×. The Linux frame-scale figures move by about ±10% from run to
run on this laptop.

## Not taken

| Change | Why |
|---|---|
| Four sub-histograms in the radix median's first pass | 0.95, for four times the histogram memory charged to every worker. |
| A byte mask in the radix median's second pass | 1.06: slower than the test it replaced. |
| `LanczosLut::at` truncation in place of `floor` | No measurable gain on x86-64-v3, where `floor` is one instruction. |
| The MRS significance fold as an explicit `Isa` kernel, with a plain store in place of the masked store x86's code generator makes of `f64::max` | No change: the masked store waited on the division before it, and the tile phase is bound by memory traffic, not by the store. |
| MRS coefficient columns zeroed in parallel | Slower: a fresh allocation's zero pages cost nothing until written, and an explicit fill writes all 32 MB. |
