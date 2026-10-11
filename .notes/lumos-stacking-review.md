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
PIP-5, PIP-19                              independent
```

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

PIP-5, PIP-6 and PIP-19 wait on [Q6](lumos-stacking-review_QUESTIONS.md#q6-how-to-measure-the-spill-tiers-disk-reads-pip-5-pip-6-pip-19): how to measure cold reads.

| ID | Question | How |
|---|---|---|
| PIP-5 | A parked frame is warped straight from a map advised `Sequential` (`frame_store/stored_plane.rs`, `pipeline/pipeline_frame.rs`). A 180° flip reads rows in reverse, a 90° turn reads one column per output row, against the readahead. The plan already charges the source as resident, so the map saves nothing. | Add legs to `pipeline/bench.rs`: Auto + spill warp at 0°, 90° and 180°, `Sequential` against `Normal`. Adopt `Normal` (or an owned read) if any angle is at least 1.2× faster and none is slower. |
| PIP-6 | Auto + spill parks the demosaiced frame (3 planes) where the calibrated mosaic (1 plane plus flags) would do, at the cost of a second demosaic. | Bench both on SSD; adopt the faster. The output is identical, because the demosaic is deterministic. |
| PIP-19 | Disk I/O and compute do not overlap: each chunk boundary is a barrier and then a burst of page faults. DSS measured reads at about 7% of combine time. | `madvise(WILLNEED)` the next chunk through `MappedFile` (`platform/mod.rs`); adopt if the spill-tier combine bench gains at least 10%. |
| RAW-10 | Every RAW load reads the whole file and opens it from memory (`io/raw/libraw.rs`, `libraw_open_buffer`). | `bench_unpack_file_vs_buffer` (`bench` + `real-data`) on CR2, CR3, NEF and RAF. *Needs real data.* |
| DMS-6 | Third-party OSC FITS (ZWO, QHY, N.I.N.A.) carry no white balance, so the demosaic's direction decisions see the sky's colour cast as structure. Siril and PixInsight also debayer unbalanced. | Gains from each colour's background, used only inside the demosaic. Compare star colour and zipper artifacts on real OSC data with and without; adopt only if it helps. *Needs real data.* |

---

## Checks left for the user

- **Batch 17:** `DECODE_PINS.raw_cfa` is a placeholder, since its digest now covers the RAW
  decode's flags and quantization σ. Run `raw_decode_snapshot` with `real-data` and pin the digest
  it prints.
- **Batch 23:** the flagged fraction per colour on a cooled-CMOS master and on a DSLR set. If it
  is large on a well-stacked cooled master, the threshold multiplier is the lever. Gr/Gb repairs.
- **Batch 27:** `k` across a DSLR session, and the residual hot-pixel pattern with and without it.
- **Batch 25:** `common/src/file_utils/platform/windows.rs` is checked only by a Windows clippy;
  run the `common` tests on Windows, `FileIdentity::of` above all.
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
