# SIMD backends — carry-forward

What is left to _do_ after the `simd::dispatch!` conversion, the constant/hygiene pass and the
layout move. The findings themselves stay in `lumos-review.md`; this file is the action list.
Delete an item once it is done.

## Verification reach on this machine

The host is aarch64-apple-darwin, so the NEON arms compile and run under the ordinary chain — no
cross toolchain, and `target_feature="neon"` is baseline on both aarch64-apple-darwin and
aarch64-unknown-linux-gnu, so nothing needs `#[target_feature(enable = "neon")]`.

The x86 arms can be reached too, which is what caught the widened SSE kernel:

    cargo test -p lumos --target x86_64-apple-darwin --lib --features internals

`--all-features` does not work for that target — `ort-sys` has no prebuilt x86_64-apple-darwin
binaries — so name the features and leave `ml` out. Rosetta reports sse4.1 but not avx2 or fma, so
that run executes the SSE4.1 rung and only compiles the AVX2 one.

## AVX2 on real hardware — measured

Both x86 items are closed, on a Ryzen 7 6800U (Zen3+, Debian; runtime detection reports avx2, fma
and sse4.1). `cargo test -p lumos --lib --all-features math::sum` passes all 15 tests there, and
because the gate admits every length at or past 8 the sweep's 8/9/16/17/63/64/65/257/1000 cases run
the AVX2 arm rather than merely compiling it. The two crossover benches did what they were built
for: they measured the gate with nothing needed but the machine.

The kernels are correct. The `sum_f32` gate is not. Backend against scalar, medians of three
interleaved rounds pinned to one core, release — the rounds agree within 1% at every length:

| n               | 5     | 6     | 7     | 8     | 9     | 10    | 11    | 16    | 32    |
| --------------- | ----- | ----- | ----- | ----- | ----- | ----- | ----- | ----- | ----- |
| `sum_f32`       | 0.60x | 0.68x | 0.76x | 0.80x | 0.92x | 1.01x | 1.12x | 1.71x | 3.60x |
| `weighted_sums` | 0.68x | 0.80x | 0.77x | 1.23x | 1.21x | 1.28x | 1.30x | 2.06x | 3.39x |

`weighted_sums` wins from the lane minimum, so its gate is right. `sum_f32` is 20% _slower_ than the
fallback at the gate itself and does not break even until 10. The mechanism the aarch64 result was
extrapolated through does not carry, because the x86 fallback is not scalar:
`scalar::sum_f32` is SSE2-vectorized on every x86_64 target, so the AVX2 kernel races a 4-wide f64
accumulation rather than a scalar loop, and one vector's worth of work does not amortize the
reduction. `scalar::weighted_sums` carries two accumulators and a multiply, which LLVM vectorizes
less well, so there the AVX2 arm is ahead immediately.

Settled: `sum_f32` now gates on `AVX2_SUM_F32_CROSSOVER = 16` on x86 and `weighted_sums` keeps the
lane minimum, so one measured crossover is back in the module. `CROSSOVER_SIZES` gained 6, 10, 12
and 24 so the bench can see the region the constant sits in — stepping 4, 8, 16 is how a losing 8
went unnoticed in the first place.

That leaves the two entry points on different rungs from 8 to 15 elements, which costs the bit-for-
bit agreement between `mean_f32` and a unit-weighted `weighted_mean_f32` inside that window. It is
not a rounding technicality: on cancelling values the two land up to ~500 f32 ULPs apart, and
`the_split_gate_window_is_where_the_two_entry_points_diverge` pins an eight-element witness two ULPs
apart so the window cannot be quietly reopened or closed. Nothing in the pipeline compares them
there — the combine only ever calls `weighted_mean_f32`, and `mean_f32`'s one production caller is
`sigma_clipped_core` — so the window is documented rather than closed. Closing it would mean gating
`weighted_sums` at 16 too and giving up 1.23-2.06x at exactly the frame counts stacks are usually
built from.

## NEON gates — measured

The NEON gates are measured, at the structural minimum of one full vector (4 f32). Backend against
scalar by length, medians, release:

| n               | 1     | 2     | 3     | 4     | 8     | 16    | 32    | 64    |
| --------------- | ----- | ----- | ----- | ----- | ----- | ----- | ----- | ----- |
| `sum_f32`       | 0.69x | 1.00x | 0.88x | 1.36x | 1.20x | 1.57x | 1.35x | 1.93x |
| `weighted_sums` | 0.88x | 0.88x | 0.88x | 1.03x | 1.34x | 1.55x | 1.65x | 1.90x |

## Open decision — delete the median9 backends?

Everything else stage 4 set out to decide has been settled, though not the way that stage expected:
`math::sum` now has no SSE rung and no crossover thresholds at all. Widening to f64 removed the
compensation the old measurements were really measuring, the SSE weighted-mean kernel was deleted
on the same argument that had already kept one out of `sum_f32`, and both gates became the
structural lane minimum — which the x86 measurement above has since walked back for `sum_f32`, whose
gate is a measured crossover again. The aarch64 measurement median9 was waiting on can be taken
here.

- [ ] Decide whether `median_filter/simd/x86/` (190 lines), `simd/neon.rs` (~90), the shared
      `median9_simd_sort!` macro (85) and `x86/tests.rs` (190) are worth carrying, given
      `bench_median_filter_dispatch_vs_scalar` puts the dispatched kernel within ±3% of the scalar
      loop at widths 64-4096. A maintenance question, not a performance one.
- [ ] Measure the NEON half on real hardware before deleting it. The x86 tie does not transfer:
      it comes from LLVM auto-vectorizing the scalar loop, and there is no guarantee it does so as
      well on aarch64.

## Open decision — one backend-file style split

- [ ] `#[allow(unsafe_op_in_unsafe_fn)]` on the function (~15 kernels) versus a whole-body
      `unsafe {}` wrapper (~29). Both satisfy the same lint; unifying means reindenting roughly 800
      lines, so pick a direction before spending the diff.
