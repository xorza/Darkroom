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
| `noise/frame/mrs_estimate/1024` | 0.28 | 0.22 | Two-pass deviation without a division per value, in parallel shares added in a fixed order; coefficients stored as columns, so a pass reads only the columns it needs; a tile builds its first layer and significance as it first smooths, with no plane cleared or copied first; the fused à trous step below. Moves bits: the corrected two-pass sum (Chan, Golub & LeVeque) replaces Welford's update, and errs less. |
| `noise/frame/mrs_estimate/4096` | 0.33 | 0.23 | Same. |
| `noise/frame/mrs_estimate_split/1024` | 0.31 | 0.26 | Same; shares of 8192 values; the gain bins measured in parallel, each on its own; the samples tagged in parallel shares. Moves bits: each bin's gain sum is added share by share. |
| `noise/frame/mrs_estimate_split/4096` | 0.33 | 0.29 | Same. |
| `wavelet/frame/atrous_smooth/step_1/2048` | 0.51 | 0.52 | Both passes fused row by row: the five source rows convolved down the columns into one row of scratch, then along it, so the frame is read once and written once and no intermediate plane exists. Each tile, denoise and HDR hold one plane less. Moves bits: the columns are convolved before the rows, the same arithmetic in the other order. |
| `wavelet/frame/atrous_smooth/step_8/2048` | 0.55 | 0.57 | Same. |
| `lstsq/factor/2000x15` | 0.28 | 0.23 | A design taller than wide is reduced by a Householder QR and its SVD taken of the square `R`, as LAPACK's `gesvd` does: the same singular values and rank, both steps backward stable. Moves bits. |
| `lstsq/factor/500x10` | 0.37 | 0.31 | Same. |
| `lstsq/solve/2000x15` | 2.34 | 2.24 | A solve now applies the reflections to its right-hand side. Every caller factors once and solves once: factor and solve together are 0.32 of before at 2000×15. |
| `noise/frame/binned_measure/1000000` | 0.88 | | The bins measured in parallel. |
| `noise/gain_bin/4096` | 0.29 | 0.60 | The 7 bounds as a fixed array, the bin counted without a branch. Same bins. |
| `statistics/median_mut/16` | 0.55 | 0.77 | Selection on integer keys in `total_cmp`'s order, where `total_cmp` mapped both operands of every comparison. A slice with no sign bit set is its own keys, which one OR over its bits finds, so it is neither written nor mapped back; any other is mapped once each way. Same medians. |
| `statistics/median_mut/4096` | 0.82 | 1.06 | Same. On arm64 the integer selection is no faster than the float one, so the sign check is a cost there. |
| `statistics/median_mad/16` | 0.64 | 0.76 | Same, and the MAD selects the deviations' bits directly: a value with its sign bit clear is its own key. |
| `statistics/median_mad/4096` | 0.81 | 1.05 | Same. |
| `statistics/mad_with_scratch/4096` | 0.81 | 1.06 | Median of magnitudes. |
| `statistics/robust_sigma_f64/4096` | 0.84 | 1.16 | Both medians on keys. |
| `statistics/sigma_clipped/16` | 0.89 | 0.99 | The values are clipped as keys across every pass; the survivors close up without a branch per value; unmapped keys read back as the values' bits. `median_fast` (a comparison that is wrong for NaN) is gone: every median is the total-order one. |
| `statistics/sigma_clipped/4096` | 0.96 | 0.90 | Same. |
| `statistics/spread/16` | 0.72 | | The MAD of a sorted window by a binary search over how many of its middle rank each side of the centre gives, the deviations two ascending runs: as many steps as the length has bits, where the merge took half the window. Same bits. |
| `statistics/spread/256` | 0.37 | | Same. |
| `pixel_quadrature/sufficient_order/gaussian_0.5/1` | 0.48 | | The orders bisected, as the bound falls with the order; each order's golden-section search ends at the first ρ whose bound is within the tolerance, which the least it would end on is at most. Same orders. The star fits run it twice a star. |
| `pixel_quadrature/sufficient_order/moffat_1.5_2.5/1` | 0.46 | | Same. |
| `statistics/frame/plane_median/radix/24000000` | 0.93 | 0.67 | `add_all` takes a run: eight keys at a time, and the second pass tests eight at once and counts only those in the middle bins. Normalization hands it every full mask word. |

The `statistics` rows are against a baseline measured again after the bench fix that batches in-place
inputs to stay in cache (`52e54d2c6`): before it, a batch of copies outgrew the cache and the
4096-value figures moved by up to 2×. The Linux frame-scale figures move by about ±10% from run to
run on this laptop.

## Not taken

| Change | Why |
|---|---|
| `LanczosLut::at` truncation in place of `floor` | No measurable gain on x86-64-v3, where `floor` is one instruction. |
| MRS coefficient columns zeroed in parallel | Slower: a fresh allocation's zero pages cost nothing until written, and an explicit fill writes all 32 MB. |
