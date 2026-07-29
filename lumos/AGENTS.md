# Lumos

Astronomical image-processing library: RAW/FITS decoding, master-frame calibration, star detection, star-pattern registration, frame stacking, drizzle reconstruction, and non-linear display stretching. CPU-bound with hand-written SIMD (AVX2 / SSE4.1 / NEON) hot paths and rayon parallelism; no GPU backend. Pixels are stored **planar** (one `imaginarium::Buffer2<f32>` per channel) and normalized to `[0, 1]`.

## Mission & scope

Lumos aims to be the **most precise and the fastest** astrophotography stacking pipeline there is, and is growing from "produce a good-looking image" toward a **science data product**: the calibrated, registered, **stacked** deep-sky image _plus_ the ancillary per-pixel quality planes (coverage, weight, variance/noise) that let a downstream tool **measure** the result — photometry, source extraction, error bars — instead of merely viewing it.

The core deliverable is still that stacked master — load → calibrate → detect → register → combine — and it always comes first. **Science-metadata extras are welcome alongside it, but only when they stay low-complexity and don't derail the core**: they should ride cheaply on data the pipeline already computes (e.g. drizzle's `weight`/`linear_variance` maps fall straight out of the `Σwᵢ`/`Σwᵢ²` the accumulator already tracks). Anything that adds significant machinery without serving either the image or its measurability is still **out of scope** and should be removed rather than carried.

**Precision and correctness outrank speed.** Both are first-class goals — the hot paths are aggressively optimized — but when the two conflict, the numerically-correct choice wins; never trade accuracy of the stacked result for throughput.

## Pipeline

A stack of telescope exposures → one calibrated, aligned, combined deep-sky image. The modules below are stages in that flow:

1. **Load / decode** (`io::image`, `io::raw`) — FITS (pure-Rust `fits-well`), camera RAW (libraw → RCD/Markesteijn demosaic), or standard formats into a planar `LinearImage`. The calibration path keeps RAW as single-channel `CfaImage` (correct before demosaic).
2. **Calibrate** (`stacking::calibration_masters`) — stack calibration frames into master dark/flat/bias/flat-dark + defect map; hot detection thresholds per-color residuals after a robust 64×64-tile dark-background fit, while cold detection reads the subtracted unfloored flat. Per light frame: dark-subtract → flat-divide → defect-correct, plus optional single-frame cosmic-ray rejection (L.A.Cosmic) on the calibrated `CfaImage` before demosaic.
3. **Detect stars** (`stacking::star_detection`) — six-stage detector → flux-sorted `Star`s with sub-pixel centroids and shape/quality metrics.
4. **Register** (`stacking::registration`) — triangle matching → RANSAC/MAGSAC++ transform fit → match recovery → optional SIP distortion → image warp into a common frame.
5. **Combine** — `stacking::combine` (statistical per-pixel combine with rejection/normalization/weighting, memory-tiered) **or** `stacking::drizzle` (Fruchter & Hook variable-pixel reconstruction for dithered/super-resolution sets).
6. **Stretch** (`stretching`, _display-domain, optional_) — map the linear stacked master to a viewable image with a non-linear tone curve (MTF/STF auto-stretch or color-preserving arcsinh), parameters auto-derived from the background. The science deliverable is the linear master from step 5; stretching is display-prep that runs strictly after all linear-domain work.

`math` (SIMD sums, robust statistics, transforms) and `concurrency` (bounded Rayon mapping, pointer safety, and reusable per-job scratch) support all stages. `lib.rs` defines the entire public surface.

## Reference docs & upstream sources

- **`src/stacking/docs/`** — best-practices reference for each pipeline stage, grounded in upstream source + cross-checked research. One doc per stage: `01-load-decode.md`, `02-calibration.md`, `03-star-detection.md`, `04-registration.md`, `05-stacking-drizzle.md`, plus `README.md`. These are descriptive references rather than the module contract; their source-comparison sections can lag implementation, while the README status table records which findings have since been resolved.
- **`scripts/clone-refs.sh`** — shallow-clones the upstream software whose functionality overlaps lumos into `.tmp/refs/<name>/` for source investigation (Read/Grep without per-file registry prompts; nothing is built or linked). `--list` prints the set, `--all` adds the large suites (RawTherapee, OpenCV, astropy, kstars, …), no arg clones the core set. Idempotent — an existing clone is skipped; delete its dir to refresh. Native deps (LibRaw, cfitsio) are pinned to `Cargo.lock` versions; everything else tracks upstream HEAD. Each entry's comment names the lumos module it informs (e.g. `sep`/`sextractor`/`photutils` → `stacking::star_detection`, `magsac`/`astroalign` → `stacking::registration`, `drizzle` → `stacking::drizzle`).
- **`.tmp/refs/`** is gitignored and persists across sessions. Run the script before reading upstream source; the `src/stacking/docs/` references were built from these clones.

## Benchmarks (quickbench)

Benches are `#[quick_bench]` fns (expand to `#[test] #[ignore]`) in per-module `bench.rs` files — e.g. `stacking/registration/resample/bench.rs`, plus RCD/Bayer/stacking. No `cargo bench` target; they run through `cargo test`.

- **Run** — always `--release` (debug numbers are meaningless and print a warning):
  ```bash
  cargo test -p lumos --release <filter> -- --ignored --nocapture
  ```
  `<filter>` is a substring of the test path: a bench name (`bench_warp_lanczos3_1k`) or a whole bench module (`interpolation::bench`). Omit it to run every bench.
- **Auto-comparison is the baseline mechanism.** Each bench writes `bench-results/<name>.txt` (gitignored); the next run prints a coloured `faster`/`SLOWER` diff against it (±5% threshold). To measure an optimization: run once (baseline) → make the change → run again, the diff is automatic. The file is overwritten each run, so to keep a baseline across several iterations, copy `bench-results/` aside first (into `.tmp/`).
- For SIMD/kernel work, prefer the **single-thread** variants (e.g. `bench_warp_lanczos3_1k_single_thread`) to isolate per-thread throughput from rayon + memory effects; the multi-thread benches show realistic end-to-end time. **aarch64 is the profiled target** (NEON mandatory; x86 SIMD is runtime-detected — see the SIMD dispatch table above).
