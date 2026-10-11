# Lumos stacking and RAW decode — implementation plan

Scope: `lumos` RAW decode (`io/raw`, `libraw-sys`), demosaic, calibration masters, the combine
engine, the pipeline and frame storage, and drizzle plus the warp that feeds the combine.
Reference projects (shallow clones in `.tmp/`): Siril, DeepSkyStacker, RawTherapee,
librtprocess, darktable, ccdproc. PixInsight, astroscrappy and DrizzlePac are compared through
their documented behaviour or source.

**Status.** Batches 1–18 are done and committed. This file holds the
open work only, with the evidence each open finding needs. The six per-area reports it replaces,
with the done findings and the lists of what was checked and found correct, are in git at
`a352d8ef1:.notes/lumos-stacking-review/`. Every open finding below was checked again against the
code at `a352d8ef1`.

Paths are relative to `lumos/src/` unless stated otherwise. Line numbers drift, so search for the
named item.

---

## How to run this plan

- **One batch per change.** Start a batch only when the tree is clean (the previous batch is
  committed). The user commits; do not commit.
- **Order.** Follow the dependency graph below. Inside a batch, follow its steps in order.
- **Verification after each batch**, from `AGENTS.md`, for each crate the batch names:
  `cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings &&
  RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps --document-private-items
  --all-features && cargo test -p <crate> --tests --all-features`, with `--features ml` in place
  of `--all-features` in `cargo test` for `lumos`. A batch that changes a public `lumos` API also
  runs `lens` and `darkroom`. A batch that changes `common` runs `--workspace`, since every member
  uses it.
- **SIMD kernels**: after a change to one,
  `cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)"
  $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter` must print 0.
- **Snapshots.** `internals/characterization` pins outputs. A batch that changes an output on
  purpose updates the pin and states why in its report. An unexplained move is a regression.
- **Measure-first items** use the benches named in the item (`--features bench`, release). Adopt
  the change only when the bench shows the gain stated as the acceptance and the output is bit
  identical, unless the item states the precision change and the user approves it.
- **Real data.** `lumos/test_data/lumos_data` is not present. Items marked *needs real data* are
  implemented and tested on synthetic data, and their real-data check goes to "Checks left for the
  user".
- **New dependencies** are only the approved ones in "Decisions".
- **Other platforms.** Only Linux builds here. Code for macOS and Windows is reported as not
  compiled unless the user runs it on the macOS test host or gives a Windows check.

## Dependency graph

```
26, 27, 28, PIP-5, PIP-19                  independent
20 ── 25
```

- 20 before 25: the kept cache moves into the ingest.

---

## Batch 20 (rest): Kept cache for the registered pipeline and the drizzle — blocked on Q5

**Done:** one `Tier`, one `FrameIntake`, admission in `ingest`, `align_and_stack_paths`, the kept
cache on every tier of `stack()` and the masters, and `CalibrationMasters::build` with its
`MasterCache` hook, which `lens` calls.

**Left (PIP-4, blocked on Q5 in `lumos-stacking-review_QUESTIONS.md`):** the registered pipeline
and the drizzle entries ignore `keep_cache`; a kept frame holds no `ImageMetadata`, which every
light needs. Per Q5, they read a light back from the kept cache before any decode, RAW runs
caching the uncalibrated `CfaImage` decode. **Test:** a rerun of a registered stack with
`keep_cache` decodes no light (count decodes through a test hook).

**Crates:** lumos, lens, darkroom.

---

## Batch 25: Kept-cache integrity — low-medium

**Finding PIP-17.** The kept cache trusts `common::FileIdentity` (length and mtime) and an
FNV-64 file stem (`frame_store/cache_key.rs` has no source path):
- a same-size rewrite that keeps the mtime (`rsync -t`, `cp -p`, a re-export) reuses stale
  planes;
- a stem collision with equal length and mtime reads another source's planes, so the
  `frame_spill.rs` doc claim "a clash rebuilds the frame, it never reads another's" is false;
- after a power loss a commit can outlive its plane contents (`PublicationMode::Cache` does not
  fsync; XFS zero-fills), and zeros pass the finiteness check.

**Design (decided: blake3 moves from `[dev-dependencies]` to `[dependencies]` of lumos).**
1. The `Commit` records the canonical source path and a blake3 digest of each file it names
   (channels, quality planes, flags, stratified samples). Reuse verifies each digest as it reads:
   the reuse pass already reads every channel (`FrameCheck::stored_samples`), and the planes it
   does not read yet are read for their digest. A mismatch is a miss.
2. A path mismatch is a miss, which makes the `frame_spill.rs` doc claim true.
3. `FileIdentity` (in `common`) adds the file id and the change time where the platform has them:
   `st_ino` and `st_ctime` on Unix; the file index and change time on Windows through
   `windows-sys` (`GetFileInformationByHandleEx`, under the `Win32_Storage_FileSystem` feature
   `common` already enables). A field the platform lacks is a documented `None`. The per-OS code
   goes to `common/src/file_utils/platform/{unix,windows}.rs`, with the `cfg` functions already in
   `common/src/file_utils/mod.rs`. Scenarium's resource cache and `lens` also compare
   `FileIdentity`: a metadata-only change (`chmod`, a rename) becomes a miss there too, a rebuild,
   never a wrong read.
4. Bump the sidecar pin.

**Tests:** a plane rewritten with the same length and mtime is a miss; a zeroed plane is a miss;
a stem collision with another path is a miss; on Unix a `chmod` of the source is a miss.

**Crates:** `--workspace`.

---

## Batch 26: Layout, visibility, API cleanup — low

| ID | Problem | Change |
|---|---|---|
| DMS-13 | `CfaPattern` lives in `io/raw/demosaic/bayer/mod.rs` and `XTransPattern` in `io/raw/demosaic/xtrans/xtrans_pattern.rs`, but FITS, calibration, defect maps and cosmic rays use them: the layering is inverted. `CfaPattern::color_at` returns `usize`, `XTransPattern`/`CfaType` return `u8`. | Move both beside `CfaType` (`io/image/cfa/{cfa_pattern.rs, xtrans_pattern.rs}`); `BayerImage` and `XTransImage` in their own files; a `CfaColour` enum for colour indices. |
| DRZ-12 | `InputMap`/`SipMap`/`Landing` sit in `drizzle/accumulator/frame_source.rs`; public `DrizzleFrame` in `accumulator/mod.rs`; free `pub(crate)` `sgarea`/`boxer` (`drizzle/geometry.rs`) and `nearest_index` (`registration/resample/kernel/mod.rs`); the same truncating floor twice (`source_position.rs`, `warp_kernel/mod.rs`); `DrizzleConfig::x2()` equals `default()`. Lanczos3 is "highest quality" and Lanczos4 "extreme quality" (`registration_config/mod.rs`); `DrizzleKernel::Lanczos` is "Best quality" although it is limited to s = p = 1 and has negative weights. | `InputMap` → `accumulator/input_map.rs`, `DrizzleFrame` → `accumulator/drizzle_frame.rs`; `boxer` → `DropQuad::overlap`, `sgarea` private; `nearest_index` → a `SourcePosition` method; one floor helper; drop `x2()`; fix the two doc claims. |
| CAL-18 | History in comments (`fits.rs`: "Version 2 stores…; version 3 keeps…"); doc comments glued to import blocks (`calibration_masters/mod.rs`, `calibration_set.rs`). | Trim; unglue. |
| CMB-11 | GESD pushes a statistic per removal into production scratch (`gesd_config.rs`, `scratch_buffers.rs`) for one test. | The NIST test recomputes the statistics through a gated helper beside `GesdConfig`; `statistics` leaves the scratch. |
| CMB-12 | `SigmaClipConfig::new(sigma, iters)` is symmetric, `LinearFitClipConfig::new(low, high, iters)` asymmetric; `max_iterations` counts all passes for σ-clip but the passes after the first for linear fit (`passes() = 1 + max_iterations`). GESD validates `alpha` in `[0, 1)`, and `alpha = 0` makes the critical value the Samuelson bound, so it never rejects; `max_outliers: Some(0)` also disables it. The GESD error names "GESD alpha" where the others name the field. | σ-clip and linear fit take `new(SigmaBounds, max_passes)` with one meaning (all passes); Winsorized, which clips once, takes `new(SigmaBounds)`; GESD validates `alpha ∈ (0, 1)` and `max_outliers ≥ 1`, and names the field. |

**Crates:** lumos, lens, darkroom.

---

## Batch 27: Dark scaling for unregulated sensors — medium (needs real data)

**Finding CAL-19.** A DSLR states no sensor temperature, so its dark is subtracted at the exposure
ratio (or 1) and the run is marked unverified. Siril offers noise-minimizing dark optimization
(`preprocess.c`, golden-section search on k ∈ [0, 2]); DSS scales by entropy or hot pixels
(`DarkFrame.cpp`). The 1 °C tolerance also accepts about 12% thermal error without a record.

**Design (decided: fit k per light on hot-pixel residuals, bias-removed darks only).**
1. **Pixels:** the defect map's hot pixels (`HotPixels`, each with its excess) not flagged in the light
   (`SATURATED`, `NO_DATA`), selected on the dark's excess `d_p` alone. Selecting on the
   regressor leaves a least-squares slope unbiased; selecting on the light's value would bias it.
   Their significance in the master keeps `d_p`'s own noise small, so the errors-in-variables
   attenuation stays small. DSS fits its dark factor on the same pixels
   (`ComputeDarkFactorFromMedian`).
2. **The light's excess** `l_p`: the raw light less the bias (through its domain map), before the
   dark and the flat, less the same local reference as `d_p` (the ring's plane through the light's
   same-colour photosites). The flat does not shade dark current, so the fit comes before it.
3. **Fit** `l_p = k·d_p` through the origin by least squares, iterated with a 3σ clip on the
   residuals' MAD-based σ, so stars, hits and unstable pixels fall out.
4. With at least 100 pixels left, `k` replaces the exposure ratio as the dark's scale in
   `LightCalibration`; otherwise the exposure ratio stands. The fit reads only those pixels and
   their rings, before the fused pass. `CalibrationOutcome` records `k`, its standard error, the
   pixel count and which scale was used.
5. **Temperature:** Batch 28 makes a temperature difference a warning. The warning stays when
   the fit corrects it; the outcome's scale source says whether it was corrected.
6. Only a bias-removed dark (`DarkBias::Removed`). A dark that holds the bias keeps today's rules.

**Tests:** a synthetic light with a dark scaled by a known `k` (1.37), a sky gradient, and stars on
some candidates recovers `k` within the derived error `σ_l / √Σd_p²`, and the starred candidates
are clipped; too few pixels falls back to the exposure ratio and says so; a light 3 °C from its dark
recovers the `k` its synthetic dark current was scaled by.

**Crates:** lumos, lens, darkroom.

---

## Batch 28: Temperature warns, never refuses — small

**Decision:** lumos does not refuse a frame or a master for its temperature. A temperature
outside `TEMPERATURE_TOLERANCE` is a warning: the run report counts it, the outcome records it,
and `tracing::warn!` logs it. No reference refuses on temperature (Siril, DSS and ccdproc read
none; PixInsight scales the dark per light "even for temperature-regulated cameras").

**Where lumos refuses today.**
- `DarkMatch::new` (`calibration_masters/dark_match.rs`) returns `DarkMismatch::Temperature` when
  a light and its dark, or a flat and its flat-dark, differ by more than 1 °C, on the sensor's
  reading where both state it, else on the camera body's.
- `CaptureConditions::check_agreement` (`frame_store/capture_conditions.rs`), called when a dark
  master is stacked (`calibration_masters/mod.rs`), returns `StackError::MasterConditions` with a
  `ConditionMismatch` when two frames of the set differ in sensor temperature.

**Design.**
1. `DarkMatch::new` compares the temperatures as today, and records a difference outside the
   tolerance instead of returning an error: `DarkMatch` and `CalibrationOutcome` carry it (the
   reading compared, the frame's and the dark's values). `DarkMismatch::Temperature` goes.
2. `RunReport` counts the lights whose dark differed in temperature, beside
   `unverified_dark_temperatures`. A flat whose flat-dark differed is recorded with the flat's
   other unverified conditions (`unverified_flat_dark`).
3. `check_agreement` checks the exposure only. A dark set whose sensor temperatures disagree
   stacks; the master states no shared temperature (`CaptureConditions::shared` already gives
   `None` then), so its lights count as unverified, and the stack's report says the set
   disagreed. `ConditionMismatch` keeps the exposure case.
4. A dark that is not scaled for temperature removes the wrong dark current, about 12% per °C. The
   warning's doc says so, and says that a fitted scale (Batch 27) corrects it.

**Tests:** a light 3 °C from its dark calibrates, and its outcome and the run report record the
difference; inside the tolerance, nothing is recorded; a dark set whose sensor temperatures span
3 °C stacks, states no temperature, and its report says the set disagreed; a dark set of two
exposures still fails with `MasterConditions`.

**Crates:** lumos, lens, darkroom (public errors and report fields change).

---

## Research items

### X-Trans cosmic-ray statistic

A statistic for single-frame cosmic-ray detection on X-Trans that is zero on a linear slope,
negative on a star's convex wings and positive at a peak: for example the residual of a robust
local plane fitted to the same-colour neighbours. Calibrate `objlim` against mono L.A.Cosmic on
Gaussian stars (σ 0.8 to 4 px, nine sub-pixel positions, every phase of the pattern), and require
no star pixel flagged at the default `objlim` and the mono path's false-positive rate on pure
noise. No reference exists to check against, so the acceptance is these synthetic tests and a
real X-Trans set. Only when it passes does `XTransUnsupported` go.

---

## Measure before you change

| ID | Question | How |
|---|---|---|
| PIP-5 | A parked frame is warped straight from a map advised `Sequential` (`frame_store/stored_plane.rs`, `pipeline/pipeline_frame.rs`). A 180° flip reads rows in reverse, a 90° turn reads one column per output row, against the readahead. The plan already charges the source as resident, so the map saves nothing. | Add legs to `pipeline/bench.rs`: Auto + spill warp at 0°, 90° and 180°, `Sequential` against `Normal`. Adopt `Normal` (or an owned read) if any angle is at least 1.2× faster and none is slower. |
| PIP-6 | Auto + spill parks the demosaiced frame (3 planes) where the calibrated mosaic (1 plane plus flags) would do, at the cost of a second demosaic. | Bench both on SSD; adopt the faster. The output is identical, because the demosaic is deterministic. |
| PIP-18 | `FrameStats::measure`'s `slot_median` collects each slot's samples, one frame across the slots in parallel: what `DECODE_TRANSIENT_FACTOR = 2` pays for, and it caps decode concurrency on tight budgets. | Replace the copy with the exact radix select (`math/statistics/radix_median.rs`) over the f32 bits in streaming passes with the same keep predicate. Adopt if bit identical; then set `DECODE_TRANSIENT_FACTOR` to the largest transient left. |
| PIP-19 | Disk I/O and compute do not overlap: each chunk boundary is a barrier and then a burst of page faults. DSS measured reads at about 7% of combine time. | `madvise(WILLNEED)` the next chunk through `MappedFile` (`platform/mod.rs`); adopt if the spill-tier combine bench gains at least 10%. |
| RAW-10 | Every RAW load reads the whole file and opens it from memory (`io/raw/libraw.rs`, `libraw_open_buffer`). | `bench_unpack_file_vs_buffer` (`bench` + `real-data`) on CR2, CR3, NEF and RAF. *Needs real data.* |
| RAW-12 | Decode makes two passes over the raw buffer: `UnpackedRaw::decode_flags` (`io/raw/unpacked_raw.rs`: a `%` and `/` per pixel, a flag plane always allocated and zero-filled, a serial count) and `normalize` (`io/raw/black_level/mod.rs`: a channel closure and black lookup per pixel). | Fuse into one row pass with per-row tables over the CFA and repeat periods, per-row flag counts, and a flag plane made only once a row flags something. Add a synthetic leg to `io/raw/bench.rs`; adopt if decode time (excluding `unpack`) falls and the output is bit identical. |
| CMB-17 | Winsorized walks the copy twice per clamp step, mean then squared deviations, up to 50 steps × N per pixel (`combine/rejection/winsorized_clip_config.rs`). | Fuse each step into one f64 pass of sums shifted by the previous centre. This is as accurate but rounds differently, and a sample at a clamp boundary can change side, so it is not bit identical. Adopt only if `bench_stack_300` Winsorized is at least 1.2× faster and the user approves the pin change, with the largest output change stated. |
| DMS-6 | Third-party OSC FITS (ZWO, QHY, N.I.N.A.) carry no white balance, so the demosaic's direction decisions see the sky's colour cast as structure. Siril and PixInsight also debayer unbalanced. | Gains from each colour's background, used only inside the demosaic. Compare star colour and zipper artifacts on real OSC data with and without; adopt only if it helps. *Needs real data.* |

---

## Checks left for the user

- **Batch 17:** `DECODE_PINS.raw_cfa` is a placeholder, since its digest now covers the RAW
  decode's flags and quantization σ. Run `raw_decode_snapshot` with `real-data` and pin the digest
  it prints.
- **Batch 23:** the flagged fraction per colour on a cooled-CMOS master and on a DSLR set. If it
  is large on a well-stacked cooled master, the threshold multiplier is the lever. Gr/Gb repairs.
- **Batch 27:** `k` across a DSLR session, and the residual hot-pixel pattern with and without it.
- **Batch 21:** `platform/macos.rs` is not compiled here (no macOS target), and
  `platform/windows.rs` only through a Windows clippy of the platform module. Build and test lumos
  on macOS and on Windows, the free space and the Job-object memory most of all.
- **RAW-10, DMS-6:** the measurements in their rows.

---

## Decisions

Each decision is also written into its batch.

| ID | Decision | Batch |
|---|---|---|
| DRZ-1 | Siril's design: drizzle each frame, then the normal combine. | 14 |
| DRZ-1 | A drizzled frame's drop weight multiplies its frame weight in the mean (Siril, DrizzlePac). | 14 |
| DRZ-6 | Only the square kernel divides by the magnification (STScI). | 14 |
| DMS-5 | CFA drizzle registers on a green proxy (Siril). | 15 |
| RAW-5 | A separate `camera_temperature`, used only when no sensor temperature exists. | 17 |
| RAW-6, RAW-15 | Refuse Phase One compressed IIQ and float DNG with typed errors. | 17 |
| RAW-8 | Saturation at `linear_max` less 0.5% of the span (RawTherapee); 95% only for `maximum`. | 17 |
| RAW-13 | OpenMP in LibRaw (approved system dependency), thread count bounded per decode slot. | 17 |
| RAW-14 | `libz-sys` (approved) and `USE_ZLIB`; drop `x3f`/`gpr`; lossy DNG refused. | 17 |
| RAW-16 | Fill `instrument`, `date_obs`, `focal_length`; refuse masters of another camera model. | 17 |
| PIP-14 | Windows Job-object limits through the `windows` crate 0.62 (approved, Windows-only). | 21 |
| CAL-5 | The hot-pixel map stays per master, from the dark alone (Siril, DSS, APP, PixInsight); the light-noise rule is withdrawn. The flagged fraction per colour is reported. | 23 |
| CAL-16 | Same-phase green repairs. | 23 |
| PIP-17 | blake3 moves to a normal dependency (approved); per-plane digests. | 25 |
| CAL-19 | Fit the dark scale per light on hot-pixel residuals (bias-removed darks), recorded. | 27 |
| DMS-6 | Measure first on real OSC data. | measure |
| CAL-19 | Never refuse for temperature, anywhere: a difference outside the tolerance is a warning in the outcome, the run report and the log (PixInsight, Siril, DSS, ccdproc). | 28 |
| CAL-7, CAL-9 | Refuse X-Trans in the single-frame cosmic-ray pass now (as Siril refuses its CFA cosmetic correction); a new X-Trans statistic is a research item. | 13 |
| DMS-5 | Normalize CFA drizzles per frame, unpaired (Siril, PixInsight), with the white noise taken out of the scale. | 15 |
