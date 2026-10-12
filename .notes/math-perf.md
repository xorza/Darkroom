# lumos math performance

Work on branch `lumos-math-perf`. Every figure is criterion's median time against the baseline
`base` (commit `ef2d3cde8`), as a ratio new / base: under 1 is faster.

## Method

- Benches: `cargo bench -p lumos --features bench -- math/...` (one target, `benches/lumos.rs`).
- A/B: the baseline is built once in its own worktree (`../darkroom-perf-base`), and every run
  saves into one shared `CRITERION_HOME`, so a later build compares to `base` with no rebuild. A
  change to the benches themselves is applied to the baseline tree too, which is then measured
  again.
- Linux: AMD Ryzen 7 6800U, 8 cores / 16 threads, x86-64-v3 build. macOS: Apple Silicon (arm64),
  8 cores.
- An optimization keeps every result at least as accurate as before. A change that moves output
  bits says so in its row.
- The tables are full runs of every bench after the last step, except where a row says otherwise.
  On the Linux laptop a full run heats it: benches whose code did not change read 1.05–1.12 in it,
  so its figures are conservative. The Mac reads 1.00 on unchanged code.

## Results

| Bench | Linux | macOS | Change |
|---|---|---|---|
| `noise/frame/mrs_estimate/1024` | 0.31 | 0.16 | The support iteration's deviation without a division per value: a two-pass sum (Chan, Golub & LeVeque) in parallel shares added in a fixed order, and after an iteration's first pass one pass each, its deviations taken from the mean the iteration before found and corrected by their sum. Coefficients stored as columns, so a pass reads only the columns it needs. A tile builds its first layer and significance as it first smooths, with no plane cleared or copied first, and smooths by the fused à trous step below. Moves bits in f64; the frame statistics' f32 output moved only with the à trous order. |
| `noise/frame/mrs_estimate/4096` | 0.30 | 0.16 | Same. |
| `noise/frame/mrs_estimate_split/1024` | 0.28 | 0.19 | Same, and the gain bins measured in parallel, each on its own, the samples tagged in parallel shares. Moves bits: each bin's gain sum is added share by share. |
| `noise/frame/mrs_estimate_split/4096` | 0.29 | 0.20 | Same. |
| `noise/frame/binned_measure/1000000` | 0.98 | 0.82 | The bins measured in parallel. |
| `noise/gain_bin/4096` | 0.29 | 0.60 | The 7 bounds as a fixed array, the bin counted without a branch. Same bins. |
| `wavelet/frame/atrous_smooth/step_1/2048` | 0.54 | 0.54 | Both passes fused row by row: the five source rows convolved down the columns into one row of scratch, then along it, so the frame is read once and written once and no intermediate plane exists. Each MRS tile, denoise and HDR hold one plane less. Moves bits: the columns are convolved before the rows. |
| `wavelet/frame/atrous_smooth/step_8/2048` | 0.59 | 0.58 | Same. |
| `lstsq/factor/2000x15` | 0.29 | 0.23 | A design taller than wide is reduced by a Householder QR and its SVD taken of the square `R`, as LAPACK's `gesvd` does: the same singular values and rank, both steps backward stable. Moves bits. |
| `lstsq/factor/500x10` | 0.38 | 0.31 | Same. |
| `lstsq/solve/2000x15` | 2.50 | 2.24 | A solve now applies the reflections to its right-hand side. Every caller factors once and solves once: the two together are 0.32 of before at 2000×15. |
| `pixel_quadrature/gauss_legendre/7` | 0.007 | 0.009 | Every order's rule solved once, on first use, and copied out after, as the Lanczos tables are: the fits take one a star. Same nodes. |
| `pixel_quadrature/gauss_legendre/16` | 0.001 | 0.002 | Same. |
| `pixel_quadrature/sufficient_order/gaussian_0.5/1` | 0.51 | 0.42 | The orders bisected, as the bound falls with the order; each order's golden-section search ends at the first ρ whose bound is within the tolerance, which the least it would end on is at most. Same orders. The star fits run it twice a star. |
| `pixel_quadrature/sufficient_order/moffat_1.5_2.5/1` | 0.48 | 0.43 | Same. |
| `statistics/median_mut/16` | 0.54 | 0.73 | Selection on integer keys in `total_cmp`'s order, where `total_cmp` mapped both operands of every comparison. A slice with no sign bit set is its own keys, which one OR over its bits finds, so it is neither written nor mapped back; any other is mapped once each way. The even count's lower middle by a fold with `Ord::max`: `Iterator::max` compiled to a scalar loop, ten times the fold's time on arm64. Same medians. |
| `statistics/median_mut/4096` | 0.80 | 0.91 | Same. |
| `statistics/median_mad/16` | 0.64 | 0.75 | Same, and the MAD selects the deviations' bits directly: a value with its sign bit clear is its own key. |
| `statistics/median_mad/4096` | 0.78 | 0.89 | Same. |
| `statistics/mad_with_scratch/4096` | 0.76 | 0.89 | Median of magnitudes. |
| `statistics/robust_sigma_f64/4096` | 0.80 | 0.95 | Both medians on keys. |
| `statistics/sigma_clipped/256` | 0.78 | 0.72 | The values clipped as keys across every pass; the survivors close up without a branch per value; unmapped keys read back as the values' bits; a pass after the first searches for its median on one side only, as the one before left the keys partitioned at its middle and the survivors keep their order. `median_fast`, whose comparison is wrong for NaN, is gone: every median is the total-order one. Figures from runs of this bench after the last step. |
| `statistics/sigma_clipped/4096` | 0.78 | 0.68 | Same. |
| `statistics/sigma_clipped/16` | 0.89 | 1.10 | Same. No caller clips so few: the star background clips up to 2048 annulus samples, the background mesh a tile. |
| `statistics/spread/16` | 0.74 | 0.98 | The MAD of a sorted window by a binary search over how many of its middle rank each side of the centre gives, the deviations two ascending runs: as many steps as the length has bits, where the merge took half the window. Same bits. |
| `statistics/spread/256` | 0.37 | 0.30 | Same. |
| `statistics/frame/plane_median/radix/24000000` | 0.91 | 0.68 | `add_all` takes a run: eight keys at a time, and the second pass tests eight at once and counts only those in the middle bins. Normalization hands it every full mask word. |
| `floor_i32/floor_i32/f32/4096` | 1.00 | 0.41 | The target's rounding instruction (`roundss` from SSE4.1, arm64's `frintm`) where it has one; truncation and one step down only on x86-64 without SSE4.1, where `floor` is a library call. Same values. |
| `floor_i32/floor_i32/f64/4096` | 0.97 | 0.67 | Same. |

Unchanged, and at 1.00 on the Mac: `sum`, `dmat3`, `lanczos`, `linear_system`, `lm_controller`,
`pixel_quadrature/integrate`.

## Not taken

| Change | Why |
|---|---|
| The MRS significance fold as an explicit `Isa` kernel, with a plain store in place of the masked store x86's code generator makes of `f64::max` | No change: the masked store waited on the division before it, and the tile phase is bound by memory traffic, not by the store. |
| Four sub-histograms in the radix median's first pass | 0.95, for four times the histogram memory charged to every worker. |
| A byte mask in the radix median's second pass | 1.06: slower than the test it replaced. |
| The radix median in one pass, bracketing the median from normalization's 65 536 samples and selecting among the values in the bracket | The bracket needs a buffer of about 2% of a plane for every worker, and normalization takes the radix path only for a partial common domain: whole planes take their median from the frame statistics. |
| Floyd–Rivest selection in place of `select_nth_unstable` | No published comparison against the standard library's branch-free introselect, and the gain is not certain against it. |
| `LanczosLut::at` truncation in place of `floor` | No measurable gain on x86-64-v3, where `floor` is one instruction. |
| MRS coefficient columns zeroed in parallel | Slower: a fresh allocation's zero pages cost nothing until written, and an explicit fill writes all 32 MB. |
