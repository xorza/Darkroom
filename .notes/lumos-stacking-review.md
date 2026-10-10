# Lumos stacking and RAW decode — implementation plan

Scope: `lumos` RAW decode (`io/raw`, `libraw-sys`), demosaic, calibration masters, the combine
engine, the pipeline and frame storage, and drizzle plus the warp that feeds the combine.
Reference projects (shallow clones in `.tmp/`): Siril, DeepSkyStacker, RawTherapee,
librtprocess, darktable, ccdproc. PixInsight, astroscrappy and DrizzlePac are compared through
their documented behaviour or source.

The detailed per-area reports, with full evidence, are in
`.notes/lumos-stacking-review/{raw_decode,demosaic,calibration,combine_math,pipeline,drizzle_warp}.md`.
Each ID (RAW-n, DMS-n, CAL-n, CMB-n, PIP-n, DRZ-n) points to an entry there. Where this plan and a
report disagree, this plan wins: it holds the decisions taken since the review. Paths are relative
to `lumos/src/` unless stated otherwise; line numbers in the reports drift, so search for the
named item.

Batches 1–12 and 14 are done and committed, and Batch 13 except its X-Trans part (Q1). Every finding below was re-checked against the code after
Batch 7 and still holds.

---

## How to run this plan

- **One batch per change.** Start a batch only when the tree is clean (the previous batch is
  committed). The user commits; do not commit.
- **Order.** Follow the dependency graph below. Inside a batch, follow its steps in order.
- **Verification after each batch**, from `AGENTS.md`:
  `cargo fmt -p lumos && cargo clippy -p lumos --all-targets --all-features -- -D warnings &&
  RUSTDOCFLAGS="-D warnings" cargo doc -p lumos --no-deps --document-private-items
  --all-features && cargo test -p lumos --tests --features ml`. Add `-p lens` / `-p darkroom`
  when a public lumos API changes. A batch that touches `libraw-sys` also runs its chain.
- **SIMD kernels** (Batches 18, 19, 22): after the change,
  `cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)"
  $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter` must print 0.
- **Snapshots.** `internals/characterization` pins outputs. A batch that changes an output on
  purpose updates the pin and states why in its report; an unexplained move is a regression.
- **Measure-first items** use the benches named in the batch (`--features bench`, release). Adopt
  the change only when the bench shows the gain stated as the acceptance and the output is bit
  identical (or the batch states the precision change).
- **Real data.** `lumos/test_data/lumos_data` is not present. Items marked *needs real data* are
  implemented and tested on synthetic data; their real-data check is left for the user, and the
  report says so.
- **New dependencies** are only the approved ones listed in "Decisions".

## Dependency graph

```
13 (X-Trans, Q1), 15, 16, 17, 22, 24, 26   independent
23 ── 27
18 ── 19
21 ── 20 ── 25
```

- 23 before 27: the dark scale is fitted on the hot candidates 23 builds.
- 21 before 20 and 25: the unified ingest and the cache integrity use the platform entity.

---

## Batch 13 (rest): X-Trans cosmic-ray statistic — blocked

**Blocked on Q1** in `lumos-stacking-review_QUESTIONS.md`. The X-Trans significance `v − median₈` is
positive on a star's convex wings, so the pass eats well-sampled stars on X-Trans at any `objlim`.
Left here: the X-Trans contrast calibration and its star-preservation test (CAL-7), and the
X-Trans local recompute (CAL-9), both of which depend on the statistic Q1 chooses. Done in Batch
13: the saturated-star mask (CAL-6), the σ normalization of the X-Trans significance with its
noise-only test (CAL-7), the detector types (CAL-17), and the mono and Bayer local recompute
(CAL-9).

---

## Batch 15: CFA (Bayer / X-Trans) drizzle — medium (feature)

**Finding:** DMS-5 / DRZ-5. Every OSC frame is interpolated twice (demosaic, then warp).

**Design:**
1. The drizzle takes a calibrated `CfaImage`. Each photosite deposits only into the plane of its
   own colour, with a per-channel weight and `q` plane (Siril `cdrizzlebox.c`: `chan =
   FC_array(...)`). Batch 14's `FrameQuality::Drizzled` holds one drop-weight plane and one
   Kish-size plane that every channel shares; this batch gives each channel its own pair, read
   per slot by `FrameGate`, `FrameCheck::stored_quality`, the frame stores and the fill gate's
   depth.
2. Registration runs on a green proxy, as Siril does (`registration/global.c`: "a copy of the orig
   image … to interpolate non green pixels"): green interpolated at the R and B sites (Bayer: mean
   of the four green neighbours; X-Trans: mean of the hex greens), never a full demosaic.
3. A light's statistics (noise per colour) come from the mosaic (`MosaicNoise`), as now.
4. RCD/Markesteijn stay the default path for undithered or few-frame sets; CFA drizzle is a
   config choice.

**Tests:** a dithered synthetic RGGB set (known scene) drizzled at s = 1, p = 1 recovers each
colour plane where every output pixel is covered; a pixel covered only by red photosites has zero
green weight; the green proxy registers a shifted pair to the same transform as the demosaiced
frames within the registration tolerance.

---

## Batch 16: Drizzle scatter performance — medium (measure first)

**Findings:** DRZ-2, DRZ-11.

**Steps:**
1. Add bench legs to `bench_drizzle_kernels`: 45°, 90°, and a SIP map. Record the baseline.
2. Limit each scanned input row to the column interval whose drops can reach the band: closed
   form for affine and homography maps; for SIP the sampled outline plus one column of margin.
3. Return the landing position first and reject on row and column before computing magnification
   or the other corners.
4. Square kernel: compute the corner lattice once per input row (shared by neighbours); for affine
   maps use `centre ± J·h`.
5. Drizzle Lanczos reads `LanczosOrder::Three.lut()`, which interpolates between entries.

**Acceptance:** the 90° leg at least 2× faster; output bit identical to before (the band-invariance
and closed-form tests pin it), except Lanczos, which moves within the table's stated 1.1e-7.

---

## Batch 17: RAW sensor classification and refusals — medium

**Findings:** RAW-3, RAW-4, RAW-5, RAW-6, RAW-7, RAW-8, RAW-13, RAW-14, RAW-15, RAW-16, RAW-17.

**Design:**
1. **RAW-3:** accept Bayer only for `colors == 3` and `cdesc == "RGBG"` (read `idata.cdesc`);
   everything else goes to `None`, and the fallback refuses `colors == 4` with a typed error.
2. **RAW-4:** shim accessors for `unpacker_data.fuji_lossless` and the CRX header
   `encType`/`imageLevels`. Canon C-RAW (lossy CRX) and Fuji lossy RAF get no quantization σ
   (`None`), since their step is not one ADU. Fix the doc comment.
3. **RAW-5 (decided):** a separate `camera_temperature` field. Calibration uses it only to match
   darks when no sensor temperature exists, and the outcome says which it used.
4. **RAW-6 and RAW-15 (decided):** refuse Phase One compressed IIQ (shim
   `is_phaseone_compressed`) and float DNG (LibRaw's float-image flag) with typed `RawError`
   variants that name the format.
5. **RAW-7 + RAW-8 (decided):** use `linear_max[c]` only when `black_c < linear_max[c] ≤ maximum`.
   Then the saturation level is `linear_max[c]` less 0.5% of the stated span
   `linear_max[c] − black_c`: RawTherapee's conservative white level, 50–100 units below the clip
   at 14 bits and 10–20 at 12 bits (`rtengine/camconst.json`, "How to Measure White Levels").
   Keep `SATURATION_FRACTION = 0.95` only for `maximum`. Correct the `pixel_flags.rs` doc claim.
6. **RAW-13 (decided, approved system dependency):** `libraw-sys/build.rs` compiles with OpenMP
   when the compiler supports it (`-fopenmp` with gcc/clang and libgomp/libomp; `/openmp` with
   MSVC). On macOS it looks for libomp (Homebrew prefix); when OpenMP is not available it builds
   without and emits a `cargo:warning`. The shim exposes `omp_set_num_threads`; each decode sets
   its thread's OpenMP count to `max(1, cores / decode_slots)` before `unpack`, so rayon slots ×
   OpenMP threads stay within the cores.
7. **RAW-14 (decided, approved dependency):** add `libz-sys` and define `USE_ZLIB` so deflate DNG
   decodes. Remove `x3f` and `gpr` from `RAW_EXTENSIONS` and the Foveon claim. Lossy DNG stays
   refused.
8. **RAW-16 (decided):** fill `instrument` (normalized make and model), `date_obs`
   (`other.timestamp`, ISO 8601 UTC) and `focal_length`; leave the pixel size empty. Calibration
   refuses a master whose `instrument` differs from the light's when both state one.
9. **RAW-17:** one identify-stage validator shared by peek and load, including the SuperCCD
   refusal; peek refuses the decoders that change `filters` inside `load_raw` (OmniVision/RPi,
   Pentax 4-shot).

**Tests:** table tests for RAW-7/8 (the bounds and the margin, per channel); RAW-3 with the F828
(`0x9c9c9c9c`, colors 4, "RGBE") and a CMYG filter word refused; RAW-16 fields read from a
synthetic metadata struct; the OpenMP thread bound as a pure function of `cores` and `slots`; the
decode pins (`DECODE_PINS`) move where RAW-8 moves a flag (state it). Formats without test files
(C-RAW, IIQ, float DNG) are covered by unit tests on the classification functions.

---

## Batch 18: Shared demosaic driver, gains inside the kernel — medium

**Findings:** DMS-11, DMS-8, DMS-4.

**Design:** one generic tiled driver (a kernel-tile trait with `bytes()`, `margin`, `border`,
`demosaic(place, out)`, plus a shared `OutputPlanes`, tile scheduling and memory accounting) and
one CFA-generic border fill over `CfaType::color_at` (the weighted 3×3 form with the radius
fallback). The white-balance gains are applied as each tile reads its input; native samples are
written from the unbalanced input, so they are exact (DMS-4); interpolated samples are unbalanced
by the reciprocal at the tile's write.

**Tests:** `rcd_all_patterns_preserve_native_samples…` extended to the production path through
`CfaImage::demosaic` with non-unit gains: native samples bit exact. Digests unchanged where gains
are 1.

**Acceptance:** `demosaic` wall time on a 24 MP bench frame falls by the removed passes; report it.

---

## Batch 19: RCD vectorization — medium (measure first)

**Findings:** DMS-7, DMS-9.

**Steps:** split the frame into per-phase half-resolution planes at copy-in; write the RCD loops as
`simd::Isa` kernels over them; compute the low-pass filter only at R/B sites; keep the high-pass
filter in ring rows; retune `TILE`. Optionally (DMS-9) compute Markesteijn's YPbPr and derivatives
row by row and retune its tile per pass count.

**Acceptance:** `bench_rcd_demosaic_core` 6000×4000 at least 1.5× faster without
`target-cpu=x86-64-v3`, and the librtprocess digests unchanged.

---

## Batch 20: One ingest stage — medium (design)

**Findings:** PIP-11, PIP-12, PIP-13, PIP-10, PIP-4, CAL-14.

**Target design:**
1. `ingest::Ingest<Source, Sink>`: one decode–admit–store loop. Sources: `Paths { paths, step:
   Option<&dyn FrameStep> }`, `Held(Vec<I>)`, `Raw { paths, masters }`, `Decoded(&[P])` (PIP-10).
   Sinks: `Park` (registration's first pass) and `Store` (straight to the combine). Both
   `FrameCache::from_paths` and `LightSource` consume it.
2. `FrameCheck` and `SetFacts` move into `ingest/`; admission gets its own error enum
   (`AdmissionError`), converted into `StackError`/`AlignStackError` at the boundary. `ingest`
   no longer imports `combine`.
3. One `Tier` enum (`Resident | Spilled { scratch: RunScratch, chunk_memory }`) in `frame_store`,
   built from the `MemoryPlan`; `CacheTier`, `FrameTier`, `LoadedTier.spilled` and the bool
   `CacheTier::of` go. The parked-light count moves to the run report.
4. PIP-13: `FrameToPark` becomes `&DetectedFrame` plus an index; `StoredImage` becomes a
   `StoredFrame` with `FrameQuality::None` and its metadata beside it; `Lights::Held` uses the
   owned-cells helper; the loader's private types fold into the ingest.
5. PIP-4: the kept cache is a decode-level memo, looked up before any decode on every tier; RAW
   runs cache the uncalibrated `CfaImage` decode (what the key describes).
6. CAL-14: `CalibrationMasters::build(CalibrationSet<&[P]>, …)` is the only master builder, with a
   per-role cache hook for `lens`; `lens` calls it.

**Tests:** every pipeline and loader test stays green with unchanged pins; a new
`align_and_stack_paths` test spills calibrated RGB FITS lights; `keep_cache` reuse on the RAM tier
skips the decode (count decodes through a test hook).

---

## Batch 21: Platform entity, statics, disk space — low-medium

**Findings:** PIP-14, PIP-15, PIP-16, PIP-22, PIP-24.

**Design:**
1. `frame_store/platform/{unix,windows}.rs` behind one `ScratchFile` type: create
   deleted-on-close, map, advise.
2. `memory/platform/{linux,macos,windows}.rs` behind one `SystemMemory` type: available memory,
   the cgroup limit on Linux, the Job-object limit on Windows, none on macOS (a documented
   `None`). Windows (decided) uses the `windows` crate 0.62 (already in `Cargo.lock`), a
   Windows-only workspace dependency with the `Win32_System_JobObjects` feature; it takes the
   smaller of the job's process and job memory limits and the free memory. `common` keeps its
   `windows-sys` use as it is.
3. PIP-15: `SystemMemory` is created per run in `RunMemory::read` (no `SYSTEM`/`GROUPS` statics);
   `COUNTER` moves onto `RunScratch`.
4. PIP-16: before a spill, estimate the bytes the tier writes (frames × `PerFrameBytes::warped`,
   plus the parked set on the Auto path) and compare with the free space of the scratch root
   (`sysinfo::Disks`, already a dependency: the disk whose mount point is the longest prefix of the
   root). Fail with `FrameStoreError::InsufficientSpace { needed, available }`. The check is
   advisory: space can still run out later.
5. PIP-22: `FramePlane::file_tag()` names cache files; `Display` stays prose.
6. PIP-24: `available_memory`, `memory_budget`, `load_concurrency` become `RunMemory` methods;
   `write_file`/`map_file` move into `stored_plane.rs`; `log_detection` moves to its user; split
   the two over-tested files.

**Tests:** the `SystemMemory` and `ScratchFile` APIs are tested where the CI runs (Linux here);
`InsufficientSpace` from a fake free-space figure through a test seam.

---

## Batch 22: Combine hot loop — medium (measure first)

**Findings:** CMB-6, CMB-17.

**Steps:** for each row tile of 64–256 px, loop the frames on the outside and transpose into a
`[tile][frame]` buffer with `simd::Isa` gain/offset, coverage mask bits, weights (the drizzle
weight from Batch 14 included) and noise terms; monomorphize away the invariant `Option`s;
precompute `1/electrons` per frame and slot; fuse each winsorized clamp step into one f64 pass.

**Acceptance:** `bench_stack_300` (large-N and ragged rows) at least 1.3× faster; output bit
identical (the per-pixel f64 accumulation order is unchanged).

---

## Batch 23: Defect-map policy — medium (needs real data)

**Findings:** CAL-5, CAL-16.

**Design (CAL-5 decided: a pixel is hot when its dark-current shot noise in the light is larger
than the light's own noise there):**
1. Hot candidates: the master dark's pixels whose bias-removed value `D_p` exceeds their colour's
   median, sorted by `D_p`. Built once per master.
2. Per run, from a representative light (the first light, calibrated without defect correction):
   - the light's background variance `σ_L²` at gain 1 (`FrameStats`, Batch 7's split);
   - electrons per unit `e`: the stated gain (`EGAIN` over a declared scale); without one, from
     the light's sky term, `e = sky / S` (`S` the sky photon variance of the split). With no flat
     the split reads `S = σ²`, which gives a smaller `e` and so a larger shot variance: the
     estimate errs toward flagging, the safe side.
3. A pixel is hot when `(D_p · t_light / t_dark) / e > σ_L²`: its dark current's shot noise alone
   exceeds the light's noise. That is a threshold on the sorted candidates, found by one binary
   search. Pixels saturated in the dark are already `NO_DATA` through `take_master_flags`.
4. The map is built per run, so a master serves runs of any exposure; cold pixels (from flats) are
   unchanged.
5. **CAL-16:** Bayer green repairs use same-phase greens only (Gr from Gr, Gb from Gb: the four at
   distance 2 and the four at 2√2). A same-phase median has no Gr/Gb bias by construction, so no
   measurement is needed to decide it.

**Tests:** a synthetic dark with known `D_p` and a light with known `σ_L²` and `e` flags exactly the
pixels above the derived threshold (hand-computed); doubling the light exposure halves the `D_p`
threshold; a Gr defect is repaired from Gr neighbours only.

**Real-data check (user):** the flagged fraction per colour on a cooled-CMOS master; Gr/Gb.

---

## Batch 24: Diagnostics — low-medium

**Findings:** PIP-8, PIP-9 / CAL-13, PIP-26, RAW-18.

**Design:** stored frames carry their input index (set at park time), and combine errors report
it. `AlignStackError::Calibration { path, source }` replaces the blanket `#[from]`. `lens`
describes `Auto` as "the frame with the lowest median FWHM among those with enough stars", and the
test `auto_reference_picks_the_richest_frame` is renamed to match. `raw_files` tests the extension
first and uses `entry.file_type()`, following a link only for RAW-named entries, with a typed error
that names the entry.

**Tests:** a registered run whose third input drops and whose fourth fails admission names input
4; a calibration error carries the path; a dangling `notes.txt` symlink does not break a RAW scan.

---

## Batch 25: Kept-cache integrity — low-medium

**Finding:** PIP-17.

**Design (decided: blake3 moves from `[dev-dependencies]` to `[dependencies]` of lumos):** the
`Commit` records the canonical source path and a blake3 digest of each plane; the reuse pass, which
already reads every plane, verifies the digest. The source identity adds inode and ctime where the
platform has them (through Batch 21's platform entity). The `frame_spill.rs` doc claim ("never reads
another's") becomes true: a path mismatch is a miss. Bump the sidecar pin.

**Tests:** a plane rewritten with the same length and mtime is a miss; a zeroed plane is a miss; a
stem collision with another path is a miss.

---

## Batch 26: Layout, visibility, API cleanup — low

| ID | Change |
|---|---|
| DMS-13 | Move `CfaPattern` and `XTransPattern` beside `CfaType` (`io/image/cfa/{cfa_pattern.rs, xtrans_pattern.rs}`); `BayerImage`/`XTransImage` in their own files; a `CfaColour` enum for colour indices. |
| DRZ-12 | `InputMap` → `accumulator/input_map.rs`, `DrizzleFrame` → `accumulator/drizzle_frame.rs`; `boxer` → `DropQuad::overlap`, `sgarea` private; `nearest_index` → a `SourcePosition` method; one floor helper; drop `x2()`; fix the two doc claims. |
| CAL-15 | `DefectMap` and its methods `pub(crate)`; fix the `crate::DefectMap` doc link; gate `count()` for tests. |
| CAL-10 | Bundle load through a memory map; `BundleError::{NotABundle, Version{found}, Checksum{hdu}, DuplicateExtension, …}`. |
| CAL-18 | Trim history comments (`fits.rs`), unglue doc comments from import blocks. |
| CMB-11 | GESD statistics leave production scratch; the NIST test recomputes them through a gated helper. |
| CMB-12 | Every rejection config `new(SigmaBounds, max_passes)` with one meaning of passes; validate GESD `alpha ∈ (0, 1)` and `max_outliers ≥ 1`. |

---

## Batch 27: Dark scaling for unregulated sensors — medium

**Finding:** CAL-19.

**Design (decided: fit k per light on hot-pixel residuals, bias-removed darks only):**
1. For each light, take Batch 23's hot candidates that are unflagged in the light and whose dark
   excess `d_p = D_p − med(D)` over the same-colour 3×3 neighbours is at least 10× the light's
   background σ, so the light's noise is small against the signal fitted.
2. The light's excess at the same pixels, `l_p = L_p − med(L)` over the same neighbours (taken
   after the bias and before the dark), is fitted as `l_p = k·d_p` by least squares, iterated with
   a 3σ clip on the residuals, so stars and hits fall out.
3. With at least 100 pixels left, `k` replaces the exposure ratio as the dark's scale in `LightCalibration`, the fused calibration kernel; otherwise
   the exposure ratio stands. The outcome records `k`, the pixel count, and which was used.
4. Only for a bias-removed dark (`DarkBias::Removed`); a dark that holds the bias keeps today's
   rules.

**Tests:** a synthetic light with a dark scaled by a known k (for example 1.37) and a sky gradient
recovers k within the derived least-squares error; too few hot pixels falls back to the exposure
ratio and says so.

**Real-data check (user):** k across a DSLR session.

---

## Measure before you change

| ID | Question | How |
|---|---|---|
| PIP-5 | A parked frame is warped straight from an `MADV_SEQUENTIAL` map; rotated reads go against the readahead. | Bench the Auto + spill warp at 0°, 90° and 180° with `Sequential` against `Normal`. Adopt `Normal` (or an owned read) if any angle is at least 1.2× faster. |
| PIP-6 | Park the 1-plane calibrated mosaic and demosaic again, instead of 3 planes. | Bench both on SSD; adopt the faster. |
| PIP-18 | Statistics copy every channel (transient factor 2). | Replace the copy with the exact radix select the combine uses (Batch 3), on the f32 bits in streaming passes; the MAD's second select runs on `|x − median|` computed on the fly. Adopt if bit identical; then set `DECODE_TRANSIENT_FACTOR` to what it then costs. |
| PIP-19 | Disk I/O and compute do not overlap. | `madvise(WILLNEED)` the next chunk through the platform entity (after Batch 21); adopt if the spill-tier combine bench gains at least 10%. |
| PIP-20 | The progress callback runs under a `Mutex` on rayon workers and can deadlock a callback that uses rayon. | Not a measurement: replace the lock with an atomic counter and an in-order sequencer that never calls out while holding a lock. Test with a callback that calls `rayon::join`. |
| RAW-10 | Every RAW load reads the file whole and opens it from memory. | `bench_unpack_file_vs_buffer` (`bench` + `real-data`) on CR2, CR3, NEF and RAF. *Needs real data.* |
| RAW-12 | Decode makes two passes over the raw buffer. | Fuse into one row pass with per-row CFA tables and incremental flag counts; adopt if decode time (excluding `unpack`) falls; output bit identical. |
| DMS-6 | Per-colour gains for third-party OSC FITS (no WB). | Compare star colour and zipper artifacts on real OSC data with and without; adopt only if it helps. *Needs real data.* |

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
| CAL-5 | Hot when the dark's shot noise in the light exceeds the light's noise; per run; `e` from the gain or the light's sky term. | 23 |
| CAL-16 | Same-phase green repairs. | 23 |
| PIP-17 | blake3 moves to a normal dependency (approved); per-plane digests. | 25 |
| CAL-19 | Fit the dark scale per light on hot-pixel residuals (bias-removed darks), recorded. | 27 |
| DMS-6 | Measure first on real OSC data. | measure |
