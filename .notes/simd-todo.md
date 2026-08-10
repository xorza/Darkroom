# SIMD backends — carry-forward

What is left to *do* after the `simd::dispatch!` conversion, the constant/hygiene pass and the
layout move. The findings themselves stay in `lumos-review.md`; this file is the action list.
Delete an item once it is done.

## Blocked — the aarch64 arms have never been compiled

There is no CI and no aarch64 hardware here, and `cargo check -p lumos --target
aarch64-unknown-linux-gnu` cannot run either: it dies in `libraw-rs-sys`'s build script for want of
an `aarch64-linux-gnu-g++`. So no NEON arm in the crate has been compiled by anything, before or
after the `dispatch!` conversion — and the `#[target_feature(enable = "neon")]` removal from
`math/sum/simd/neon.rs` and `median_filter/simd/neon.rs` is reasoned, not compiled. Every NEON file
was also moved and had its module paths rewritten during the layout stage, unverified.

- [ ] Install the cross toolchain (`aarch64-linux-gnu-gcc`, which brings the C++ driver
      `cc-rs` is looking for), and record it in `~/SYSTEM-CHANGES.md`.
- [ ] Append `cargo check -p lumos --target aarch64-unknown-linux-gnu --all-targets --all-features`
      to the verification chain for any change that touches a SIMD backend.

## Open decision — delete the median9 backends?

Everything else stage 4 set out to decide has been measured and settled: the weighted-mean SSE rung
earns its place (~1.35x over scalar above n=128), the shared 128 threshold is right for both the 8-
and the 4-lane kernel, and `sum_f32` deliberately has no SSE rung — the note on the function says
why. Only median9 is open, and it needs an aarch64 machine.

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
