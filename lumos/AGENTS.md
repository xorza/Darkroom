# Lumos

Astronomical stacking pipeline: **load / decode (RAW, FITS) → calibrate →
detect stars → register → combine (stack or drizzle)**, then an optional
non-linear **stretch**, strictly after all linear-domain work. CPU only:
vector kernels written once over `simd::Isa` (AVX2+FMA, NEON, a portable
fallback; see `src/simd/mod.rs`) and rayon. Pixels are **planar** f32, one
plane per channel, normalized to `[0, 1]`.

## Scope

The **most precise and the fastest** stacking pipeline, growing toward a
**science data product**: the linear master plus per-pixel quality planes
(coverage, weight, variance) a downstream tool measures it with — photometry,
source extraction, error bars.

- **The stacked master comes first.** A science extra must stay simple and
  ride on data the pipeline already computes. Machinery that serves neither
  the image nor its measurement is removed, not carried.
- **Thin-plate-spline distortion (`registration/distortion/tps/`) and drizzle
  (`drizzle/`) stay**, even without a production caller.
- **Precision outranks speed.** Never trade the stacked result's accuracy for
  throughput.

## Frames and files

- **A frame holds imaginarium's planes** (`PlanarImage`, `PlanarDesc`), with
  lumos's metadata, flags and flat gain. lumos keeps its own side limit,
  `frame_desc::MAX_SIDE`, which its `i32` pixel lanes set: `LinearImage` and
  `CfaImage` assert it, and every reader of an untrusted size checks it.
- **A lumos decode hands back planes.** `LoadCtx::preview` returns a
  `LinearImage` for every format: lumos's pixels are planar, and a preview is
  told from a science load by the method and the provenance it records.
  Refused: `PreviewImage` of a planar or an interleaved arm, a second copy of
  lens's wire type. Cost: a raster preview is interleaved again for an
  imaginarium op or the previewer, as a FITS or RAW one is, without loss.
- **`image/` groups by subject**: `metadata/` for what a frame carries beside
  its samples, `flags/` for the quality flags, `cfa/` and `flat_gain/`, and
  the frame types at the top.
- **A file sits with the type it stores.** The calibration bundle is in
  `calibration`, the product file in `stack_product`; the FITS extensions both
  write stay in `io::fits`. Each type reads and writes through its own
  methods, as `LinearImage::write_fits` and `StackProduct::read_fits`.
  Refused: the `impl` blocks in `io`, which break one struct per file; free
  functions, which `lumos::writer` was.
- **The identity of a decode lives with the decoders**: `io::decoder` holds
  the `Decoder` trait and the `DecoderKind`, `DecodePins` and `DecodeParams`
  that name it.
- **The camera RAW format is `libraw`'s.** `io::raw` maps LibRaw's facts into
  a `CfaImage` — the pattern, the black levels and their flags, the
  saturation, the white balance, the metadata; which codecs are lossy is
  lumos's reading of them (`RawDecoder::classify`).

## Memory

- **The memory budget is soft: it steers caching and spilling, and never refuses
  work.** A run reads the memory it plans for once, at its entry
  (`IngestConfig::memory_override`, else what the system reports available),
  and `Budget` takes 75 % of it. `RunPlan` decides from the budget whether
  frames stay resident or spill to the run's scratch, and how wide each stage
  runs: it trades memory for disk I/O. A stage that needs more than the budget
  runs anyway; it logs its overcommit and continues.
- **No decode or stage is refused for its size.** Nothing compares what a file
  or a stage needs with the budget or with the system's memory. An allocation
  the system cannot back stops the process. Refused: a ceiling of 75 % of the
  available memory, which blocked files the machine could load, and one of the
  physical RAM.

## Verification

`cargo drive check`, from the darkroom root (its Verification section says
what it runs). Tests leave out `real-data`, as darkroom's
`[workspace.metadata.drive]` says.

`real-data` runs the tests that read the gitignored ~7.4 GB dataset in
`test_data/lumos_data/`, only when asked. Its ML tests skip without ONNX
weights (`STARNET2_ONNX` / `DEEPSNR_ONNX`, or the defaults in `test_data/`).

A change to a vector kernel (anything under `simd::Isa`) also checks that
nothing fell out of line: a function the `#[target_feature]` entry does not
inline runs without AVX2 and FMA below x86-64-v3, an order of magnitude
slower, and no test fails. This must print `0`:

```
cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)" $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter
```

It catches a missing `#[inline(always)]` and a closure that calls a vector
op. Plain AVX intrinsics prove nothing: the workspace builds for x86-64-v3
(`.cargo/config.toml`). That flag may be turned off, so keep the runtime ISA
dispatch, and every kernel correct and fast without it.

## Benches

Criterion benches live in `bench.rs` beside the code they measure and run
from the one target `benches/lumos.rs` through `bench::run`:
`cargo drive bench <filter>`, `--test` to run each once.
`src/bench/mod.rs` states their shape.
