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

- [ ] Run `math::sum`'s AVX2 backend on real AVX2 hardware. It compiles and passes the suite as
      x86_64, but Rosetta offers no AVX2, so nothing here has executed it since it was widened to
      f64 — and the SSE4.1 twin that used to stand in for it has been deleted.
- [ ] Confirm `math::sum`'s AVX2 gate on x86 hardware. Both crossover constants are gone; the gate
      is now the structural lane minimum on every architecture. That is _measured_ on aarch64 (see
      below) and inferred on x86 from the same mechanism, not measured. `bench_sum_f32_crossover`
      and `bench_weighted_sums_crossover` bench the backend by name on both architectures, so the
      measurement only needs the machine.

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
structural lane minimum. Only median9 is open, and the aarch64 measurement it was waiting on can
now be taken here.

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
