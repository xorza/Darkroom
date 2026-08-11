# SIMD backends — carry-forward

What is left to *do* after the `simd::dispatch!` conversion, the constant/hygiene pass and the
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
      is now the structural lane minimum on every architecture. That is *measured* on aarch64 (see
      below) and inferred on x86 from the same mechanism, not measured. `bench_sum_f32_crossover`
      and `bench_weighted_sums_crossover` bench the backend by name on both architectures, so the
      measurement only needs the machine.

The NEON gates are measured, at the structural minimum of one full vector (4 f32). Backend against
scalar by length, medians, release:

| n              | 1     | 2     | 3     | 4     | 8     | 16    | 32    | 64    |
|----------------|-------|-------|-------|-------|-------|-------|-------|-------|
| `sum_f32`      | 0.69x | 1.00x | 0.88x | 1.36x | 1.20x | 1.57x | 1.35x | 1.93x |
| `weighted_sums`| 0.88x | 0.88x | 0.88x | 1.03x | 1.34x | 1.55x | 1.65x | 1.90x |

Both lose under four elements and win from four up, so the gate is where the win starts rather than
somewhere above it. The rows below the gate are informational — dispatch never takes the vector arm
there.

The NEON half is measured and settled for both functions. Widening to f64 made them ~3x faster,
not slower: dropping the f32 Kahan step removes four dependent ops per accumulate and its serial
chain, which more than pays for the halved lanes. At 10k elements, release, with the untouched
scalar label as the drift control (it moved 0.7% and 0.0% across the two builds):

| bench                    | f64    | f32-Kahan | scalar |
|--------------------------|--------|-----------|--------|
| `bench_weighted_mean_f32`| 1.71µs | 5.17µs    | 5.67µs |
| `bench_sum_f32`          | 1.46µs | 4.83µs    | 4.83µs |

Both compensated kernels were near-worthless over the fallback — the weighted mean within 10% of
scalar, and `sum_f32` dead level with it to three digits. The comment claiming one full vector was
enough for the NEON sum to win had never been measured; it was wrong.

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

## Benchmarking notes for whoever picks this up

These were taken on the Linux x86_64 13980HX box. The notes below about `taskset`, P/E-core
pinning, `/sys/devices/cpu_core/cpus` and the AVX2 labels apply to that machine, not to the
aarch64-apple-darwin host, which has no `taskset` and a different core-type story.

- Interleave the A/B rounds. This machine drifts far enough between runs that a single
  before/after pair is worthless — `bench_warp_bilinear_2k` swung 1.7-4.1ms across runs of
  identical code.
- Prefer whole-image or single-threaded benches. Every microsecond-scale per-row sweep attempted
  here produced results that changed shape when unrelated sizes were added to the sweep; one
  reported both "AVX2 8x faster" and "AVX2 3x slower" at the same width.
- Remember the fallbacks are not scalar. `median_filter_row_scalar` runs ~2.4 cycles per interior
  pixel and `scalar::sum_f32` is SSE2-vectorized on every x86_64 target, because SSE2 is baseline.
  A new hand-written kernel competes with an auto-vectorized one, so the bar is far higher than
  lane count suggests.
- **Pin the process to a P-core: `taskset -c 2 <bench-binary>`.** This is a 13980HX; P-cores are
  0-15, E-cores 16-31 (`/sys/devices/cpu_core/cpus`). An unpinned bench lands on either kind and
  stays there for the whole process, so runs cluster into two modes rather than scattering — which
  reads as a reproducible regression when interleaved A/B happens to sort the builds across modes.
  On `bench_measure_star_batch_6k_10000` the E-core mode costs 1.8x on the scalar weighted-moments
  path and 2.3-3.0x on the AVX2 fit paths, and it is what produced a phantom "37% gaussian
  regression" that pinned measurement showed to be 1.6%. Pinned, the same bench repeats to 0.4%.
- Take the binary path from `cargo test --no-run --message-format json` and run it directly, so
  cargo's own scheduling is out of the timing and both A/B binaries can be kept side by side.
- The centroid AVX2 fit kernels move 20-35% on refactors that change no arithmetic. At
  `81595a2c9` the `gaussian_fit` label sat at ~160ms against ~120ms both before and after it, and
  the later `e23f7350d` restructuring brought it back with no change to the kernel or its inputs.
  Treat any sub-10% delta on these benches as unattributable, and re-measure at HEAD rather than
  trusting a figure taken mid-series.
- Pinning cuts variance but does not remove it. Two back-to-back runs of one binary repeat to
  0.4%; the same binary across an hour of session spans 114-124ms on `gaussian_fit`. Interleave
  A/B within a single batch and discard batches whose spread exceeds the effect being measured.
