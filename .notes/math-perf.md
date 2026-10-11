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
| `noise/frame/mrs_estimate/1024` | 0.38 | | Two-pass deviation without a division per value, in parallel shares added in a fixed order; coefficients stored as columns, so a pass reads only the columns it needs. Moves bits: the corrected two-pass sum (Chan, Golub & LeVeque) replaces Welford's update, and errs less. |
| `noise/frame/mrs_estimate/4096` | 0.38 | | Same. |
| `noise/frame/mrs_estimate_split/1024` | 0.45 | | Same, and shares of 8192 values, so one gain bin still spreads over every worker. |
| `noise/frame/mrs_estimate_split/4096` | 0.44 | | Same. |
| `noise/gain_bin/4096` | 0.29 | | The 7 bounds as a fixed array, the bin counted without a branch. Same bins. |
| `statistics/median_mut/16` | 0.59 | | Selection on integer keys in `total_cmp`'s order, mapped once each way, where `total_cmp` mapped both operands of every comparison. Same medians. |
| `statistics/median_mut/4096` | 0.89 | | Same. |
| `statistics/median_mad/16` | 0.68 | | Same, and the MAD selects the deviations' bits directly: a value with its sign bit clear is its own key. |
| `statistics/median_mad/4096` | 0.88 | | Same. |
| `statistics/mad_with_scratch/16` | 0.62 | | Median of magnitudes. |
| `statistics/robust_sigma_f64/16` | 0.64 | | Both medians on keys. |
| `statistics/sigma_clipped/4096` | 1.04 | | `median_fast` (a comparison that is wrong for NaN) is gone: every median is the total-order one. The clip still maps its values each pass; next to fix. |

## Not taken

| Change | Why |
|---|---|
| `LanczosLut::at` truncation in place of `floor` | No measurable gain on x86-64-v3, where `floor` is one instruction. |
