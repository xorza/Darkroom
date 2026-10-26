# simd operations: performance journal

Target: the `simd` crate and the kernels built on it, in imaginarium (convert, blend,
contrast_brightness, affine_warp) and lumos (sum, convolution, threshold mask, median filter,
centroid, background mesh, demosaic, wavelet, stretch, warp). Branch `perf-simd` from
`3e68f1aa1`, against `master` (`dbfb50926`).

## Hosts

- **x86**: AMD Ryzen 7 6800U (Zen 3+), 8 cores, 27 GiB. Atomic cases pinned to CPU 2 with
  `taskset`; integration cases on every core.
- **Mac**: Apple M2 (4 P + 4 E cores), 8 GiB, fanless, macOS: no core pinning; runs drift by a
  few percent with temperature, so a Mac verdict needs three rounds.

## Build RAM peaks

- x86, `cargo drive bench` (darkroom, imaginarium and lumos bench binaries, fat LTO): 11.4 to
  15.3 GiB system-wide used, from about 6 GiB idle. One build at a time.
- Mac, the same with `CARGO_BUILD_JOBS=4`: fits in 8 GiB; one build at a time, none while it
  benches.

## Noise floors

- **x86**, atomic cases pinned to one core, min of two runs a side: A/A within ±1.5 %.
- **Mac**, integration convert cases, base first in every round: A/A up to +40 % on the second
  side, the fanless M2 heating through a round. Fixed by alternating the order (ABBA) and 20 s of
  idle before each run; a Mac verdict takes three rounds.

## Current numbers

Branch start `3e68f1aa1` against master `dbfb50926`, min of two runs a side:

- **x86**: 226 cases — 76 faster, 147 within ±4 %, 3 slower: `atomic/contrast_brightness`
  `mono_f32` +5.6 % (0.29 → 0.31 µs), `atomic/convert` `rgba_u8/rgb_u8` +6.9 % (0.37 → 0.39 µs),
  and lumos's scalar `threshold_mask/row/scalar/6000` +13 %, a reference no production path runs.
- **Mac**, ABBA, three rounds: 226 cases — 54 faster, 131 within ±4 %, 41 slower. In cache:
  contrast `rgb_f32` +52 % and `mono_f32` +8 % (the exact clamp, see the ideas), affine RGBA
  `f32` +12 % and `u16` +6 %, convert `mono_u8/mono_f32` +14 % and `rgb_u8/rgba_u8` +12 %. At full
  frame: affine RGBA `u16` +19 %, blend `rgba_u8` +24 %, and conversions of `rgba_u16`,
  `rgba_f32` and `mono_u8` sources +4 to +69 %, scalar paths among them, within the Mac's
  full-frame A/A of up to +50 %; lumos's lanczos3 warps +6 to +10 %.

Final, tip `94dbf4eee` against master, time ratio min / median / max per group:

- **x86**, atomic pinned, two rounds: affine 0.43 / 0.50 / 0.57, blend 0.48 / 0.53 / 0.75,
  contrast 0.70 / 0.99 / 1.06, convert 0.59 / 0.96 / 1.14; full frame affine 0.45 / 0.96 / 1.00,
  blend 0.34 / 0.83 / 1.00, contrast and convert 0.98–1.01. Above 1.04 only four sub-µs cases whose
  code this loop left alone (contrast `mono_f32`, convert `rgba_u16/rgba_u8`, `rgba_u8/rgb_u8`,
  `rgba_u8/rgba_u16`), within the cross-build noise.
- **Mac**, ABBA, three rounds: atomic affine 0.77 / 0.92 / 1.14, blend 0.85 / 0.90 / 0.95,
  contrast 0.72 / 0.87 / 1.19, convert 0.89 / 1.00 / 1.11. Above 1.04 in cache: affine RGBA `f32`
  1.14 and `u16` 1.07 (the redesign), contrast `rgb_f32` 1.19 (from 1.52; the exact clamp's
  compare-select), convert `mono_u8/rgb_u8` 1.10 and `rgb_u8/rgba_u8` 1.11 (the store is not the
  cause, see the ideas). Full frames 0.24–1.66, within the Mac's full-frame noise.

## Ideas

- **Exact quotient by the full scale without a divide.** The `u8`/`u16` → `f32` paths of blend
  and convert divide by 255 or 65535 for bit-exactness; `vdivps` and its neighbours lead the blend
  RGBA `u8` profile. The reciprocal's product and one fused correction, `q + r · (v − q · d)`,
  give the correctly rounded quotient for every integer of `[0, 255]` and `[0, 65535]`: checked
  exhaustively in exact rational arithmetic (the product alone errs on 126 and 512, as the code
  comments say), and by a test on every tier. Built as candidate 1.
  - x86 candidate 1, blend and convert: convert `u8`/`u16` → `f32` 0.63–0.69 in cache, parity at
    full frame; blend `u8` 1.00–1.08 and `u16` 1.08–1.16 slower. Blend's float pipes are busy,
    and the divider runs beside them: the divide's work moved onto the busy pipes. **Kept for
    convert, rejected for blend.**
  - x86 candidate 2, convert alone: 0.64–0.70 for the six pairs. Unchanged code moved ±5–20 %
    between the two builds (blend `f32` 0.89–0.95, sub-0.5 µs `u16`/`u8` pairs 1.04–1.21): the
    cross-build noise floor of the shortest kernels, which an A/A of one binary does not show.
- **A pair gather for RGBA `u8` affine taps.** Not tried: pinned to one core the warp takes about
  1.8 cycles a pixel, so its four `vpgatherdd` a block are already cheaper than the 16 loads and
  24 shuffles a load-and-transpose would take.
- **NEON `fmin` for a positive bound.** The NEON `f32` contrast loop is 12 vector instructions a
  group, each `min` and `max` a compare and a bit select. For a bound positive in every lane
  (`+∞` included) `fmin` is the specified compare-select, a NaN included; only a NaN or zero bound
  differs. `F32x8::min_positive` states that contract, NEON takes `fmin`: 10 instructions a group
  in contrast's and blend's `f32` clamp. Built as candidate 3, with candidate 2, for the Mac.
- **Kept, commit `2157e92a5`: `F32x8::div_scale`**, the corrected reciprocal on AVX2 alone. Kept
  set against base: x86 convert to `f32` 0.63–0.70; M2 `u8` → `f32` 0.87–0.94 (the widening byte
  load), `u16` → `f32` at parity.
- **Kept, commit `94dbf4eee`: `F32x8::min_positive`**, NEON's `fmin`. M2: contrast `f32`
  0.83–0.84, blend `f32` 0.89–0.94; x86 unchanged.
- **NEON `append_fourth_u8` as master's `ld3` + `st4`.** M2 `rgb_u8/rgba_u8` and `mono_u8/rgba_u8`
  1.007–1.012: neutral. **Rejected.** The +12 % against master in this case is not the store.
- **No array loop over the pair gather's samples, NEON and AVX2.** The NEON `f32` RGBA affine step
  holds 75 vector stack accesses in about 560 instructions; naming the eight samples and their
  masks instead of looping changed nothing measurable: M2 affine 0.989–1.010, x86 0.985–1.002.
  **Rejected**: the spills are not on the critical path.
- **NEON RGBA `f32` affine in pixel layout.** A redesign: a kernel shape per Isa, against
  `simd/AGENTS.md`. Spiked: M2 in cache 0.816 of base, below master; full frame 0.953. Written up
  in `.notes/perf-redesigns-simd.md`, branch `spike-neon-pixel-mix`.
- **lumos's kernels.** Profiled on x86 (convolution, median filter, threshold mask): inlined into
  their loops, no call left out of line, at parity with master on both hosts. No idea left.
