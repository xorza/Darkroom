# Lumos stacking and RAW decode — implementation plan

Scope: `lumos` RAW decode (`io/raw`, `libraw-sys`), demosaic, calibration masters, the combine
engine, the pipeline and frame storage, and drizzle plus the warp that feeds the combine.
Reference projects (shallow clones in `.tmp/`): Siril, DeepSkyStacker, RawTherapee,
librtprocess, darktable, ccdproc. PixInsight, astroscrappy and DrizzlePac are compared through
their documented behaviour or source.

**Status.** Batches 1–14 and 16–18 are done and committed, and Batch 15 except its
normalization above scale 1. This file holds the
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
- **SIMD kernels** (Batches 19, 22): after the change,
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
15r, 19r, 22, 24, 26, 28                  independent
20 ── 21 ── PIP-5, PIP-19
20 ── 25
23 ── 27
```

- 20 before 21: the disk-space check goes where the one `Tier` is built, which Batch 20 makes.
- 20 before 25: the kept cache moves into the ingest.
- 21 before PIP-5 and PIP-19: both change memory-map advice through `ScratchFile`.
- 23 before 27: the dark scale is fitted on the hot pixels 23 stores with their excess.

---

## Batch 15 (rest): CFA drizzle normalization above scale 1 — medium

**Decision:** per-frame statistics, unpaired, with the white noise taken out of the scale.

**Finding.** A CFA-drizzled frame reaches each colour at only part of the output grid: its red
and blue drops sit on a lattice of `2s` output pixels. The combine normalizes by pixel pairs
(`photometric_gain`'s errors-in-variables fit, or a ratio of medians over shared pixels). The
pixels every frame reached thin out with every frame added, so Batch 15 measures each frame
against the reference over the pixels both reached in the slot's channel (`pairwise_norms` in
`combine/normalization/mod.rs`). At scale 1 a sub-pixel dither shares every pixel. Above scale 1,
two frames about one photosite apart put one's red where the other's green fell and share no red
pixel. The normalized combine then fails with `NoCommonCoverage`: loud, not wrong. Eight frames
of random dither at scale 2, pixfrac 0.8, fail this way under `StackConfig::light()`.

**Practice.** PixInsight takes the normalization of a drizzle from ImageIntegration on the
registered, interpolated frames, with per-frame location and scale estimates, not pixel pairs.
Siril's default normalization (`src/stacking/normalization.c`, `compute_normalization`) measures
each frame alone, location and scale by IKSS (or `1.5·MAD` in its fast mode) over the frame's
non-zero pixels, which leaves out the pixels a drizzle did not reach. Siril pairs frames only in
its optional overlap normalization for mosaics (`compute_normalization_overlaps`), which needs
more than 3 shared pixels per pair: the same limit as lumos's pairs. Both references keep the
noise inside the scale estimate, so a noisier frame reads a larger scale.

**Design:**
1. For frames gathered per channel (CFA drizzle), normalization measures each frame alone, per
   colour: location from the frame's per-colour source median (`FrameStats::medians`); scale
   `√(MAD² − σ²)` from a per-colour robust scale of the source mosaic, with `σ` the measured white
   noise (`FrameStats::noise`). `FrameStats` adds the per-colour MAD.
2. Global: gain = the reference's scale over the frame's, offset from the medians.
   Multiplicative: the ratio of the source medians, as for unwarped frames. Pairs stay for every
   other set, where they are exact.
3. `√(MAD² − σ²)` is exact for Gaussian parts only, and the doc says so. A frame whose `MAD² ≤ σ²`
   has no signal structure to scale by and fails with a new `StackError` variant that names it.

**Tests:** a normalized CFA drizzle at scale 2, pixfrac 0.8, eight frames of random dither with
known per-frame gains and offsets recovers each within the estimator's derived error; a frame of
pure noise fails with the new error; a normalized CFA drizzle at scale 1 keeps its pins.

**Crates:** lumos, lens, darkroom (a public error changes).

---

## Batch 19 (rest): Markesteijn tile tuning — small (measure first, optional)

**Finding DMS-9.** `io/raw/demosaic/xtrans/markesteijn/mod.rs`: `TILE = 96` with a margin of 15
at three passes computes `(96/66)² ≈ 2.12×` its output (`(96/78)² ≈ 1.51×` at one pass). Its
`rgb` buffer holds all 8 directions at once, `8 · 96² · 12 B ≈ 884 KB` as `[f32; 3]`, more than
L2. The tile size is a measured optimum of that layout (703 ms against 866 ms at librtprocess's
114); the lever left is the footprint. The RCD part (DMS-7) is done.

**Steps:** compute YPbPr and the derivatives row by row from `rgb` instead of storing full
planes, keep only the RGB the final blend reads, and retune `TILE` per pass count.

**Acceptance:** `bench_cfa_demosaic_balanced` X-Trans 6240×4160 at least 10% faster, and the
librtprocess digests unchanged.

**Crates:** lumos.

---

## Batch 20: One ingest stage — medium (design)

**Findings:** PIP-11, PIP-12, PIP-13, PIP-10, PIP-4, CAL-14, and the issue-log entry on
`stack()` of RAW paths as `LinearImage`.

**Problems.**
- **PIP-11.** Three decode–admit–store loops. `combine/cache/loader/mod.rs`: peek →
  `MemoryPlan::plan` → `FrameAdmission` → `try_par_map_limited` → RAM, scratch or kept store.
  `pipeline/light_source.rs`: peek → `MemoryPlan::plan` or `single_pass` → `FrameAdmission` →
  `DetectorPool::try_map` → `FrameTier`. Since Batch 14, `drizzle/stack.rs`: a lazy serial load
  → drizzle → `DrizzleTier::store`, with its own finiteness check
  (`DrizzleError::NonFiniteSample`) in place of `FrameAdmission`. Each fix lands two or three
  times, and admission semantics differ. `ingest/frame_admission.rs` and
  `ingest/frame_step.rs` import
  `combine::cache::{frame_check, set_facts}` and `combine::error::StackError`, while the loader
  imports `ingest`: a cycle.
- **PIP-12.** The tier decision has five forms: `MemoryPlan::fits_in_ram: bool`;
  `pipeline/frame_tier.rs` `StagePlan` and `FrameTier::{Ram, Spill { scratch, chunk_memory,
  parked }}`; `combine/cache/core.rs` `CacheTier::{Resident, Spilled { chunk_memory }}` with
  `CacheTier::of(spilled: bool, …)`; the loader's `LoadedTier.spilled: bool`; and
  `drizzle/drizzle_tier.rs` `DrizzleTier::{Ram, Spill { scratch, chunk_memory }}` with
  `cache_tier()`. Each conversion is a place where `chunk_memory` can disagree.
- **PIP-13.** Frame carriers multiply: `FrameToPark` duplicates `DetectedFrame`; `StoredImage`
  (channels, flags, metadata) duplicates `StoredFrame`; `Lights::Held` re-implements
  `concurrency::try_par_map_bounded_owned` with `Mutex<Option<_>>` cells; the loader has nine
  private types (`LoadedTier`, `LoadedCache`, `EarlyDecode`, `Decoded`, `CachedSource`,
  `CheckedImage`, `LoadedMemoryFrame`, `TierLoad`, `FrameDiskCache`).
- **PIP-10.** Calibrated lights on disk (Siril or PixInsight FITS) have no tiered entry:
  `align_and_stack` takes `Vec<LinearImage>`, so the caller decodes every frame, the inputs stay
  resident and never spill. The `calibrate_align_stack` doc sends such users there.
- **PIP-4.** `keep_cache` works only for `stack()` and `stack_cfa_master` on the spill tier. The
  loader's RAM branch never opens the cache, so a rerun that fits in RAM decodes every frame again
  and a `keep_cache` run that fits writes nothing. `calibrate_align_stack`, `align_and_stack` and
  the drizzle entries ignore the flag. The `IngestConfig::keep_cache` doc promises more.
- **CAL-14.** The master-building rule (each role stacked under its preset; flats take the
  flat-dark, else the bias, per frame; then `from_images`) is written twice:
  `calibration_masters::internals::masters_from_files` and `lens/src/astro/nodes/calibration.rs`.
- **Issue log.** `stack()` of camera RAW paths as `LinearImage`: `FramePeek::of_decoded` counts
  `decoder_bytes: 0`, so the memory plan leaves out LibRaw's file buffer and unpacked raw, and
  frame 0 is decoded before the plan.

**Design.**
1. `ingest::Ingest<Source, Sink>`: one peek → plan → decode → admit → store loop. Sources:
   `Paths { paths, decoder, step: Option<&dyn FrameStep> }`, which serves `stack()` and PIP-10;
   `Held(Vec<I>)`; `Raw { paths, masters, cosmic_ray }`. Sinks: `Park` (registration's first
   pass) and `Store` (straight to the combine). `FrameCache::from_paths`, `LightSource` and the
   drizzle entries consume it. The drizzle entries keep one frame in drizzle at a time (each
   drizzle is parallel inside), and their stored size comes from the drizzle output, as
   `DrizzleTier::for_run` counts it today.
2. Every source peeks before any decode. A RAW path decoded as `LinearImage` peeks through
   `raw_cfa_frame_info` and counts `CfaFrameInfo::decoder_bytes`, as RAW mosaics already do. Then
   delete the issue-log entry.
3. `FrameCheck` and `SetFacts` move into `ingest/`. Admission has its own error enum
   (`AdmissionError`), converted into `StackError`, `AlignStackError` and `DrizzleError` at each
   boundary. The drizzle entries are admitted like the others, and their own finiteness check
   goes. `ingest` no longer imports `combine`.
4. One `Tier` enum (`Resident | Spilled { scratch: RunScratch, chunk_memory }`) in
   `frame_store`, built from the `MemoryPlan` in one place. `CacheTier`, `FrameTier`,
   `DrizzleTier`, `LoadedTier.spilled` and `CacheTier::of(bool)` go. The parked-light count moves
   to the run report.
5. PIP-13: `FrameToPark` becomes `&DetectedFrame` plus an index; `StoredImage` becomes a
   `StoredFrame` with `FrameQuality::None`, its metadata beside it in `PipelineFrame::Spilled`;
   `Lights::Held` uses `try_par_map_bounded_owned`; the loader's private types fold into the
   ingest.
6. PIP-4: the kept cache is a decode-level memo, looked up before any decode on every tier:
   mapped on the spill tier, read into RAM on the RAM tier. RAW runs cache the uncalibrated
   `CfaImage` decode, which is what the key describes, before the masters apply. Every entry
   honours `keep_cache`.
7. PIP-10: a public `align_and_stack_paths` over calibrated linear files, decoded on workers
   through the `Paths` source. Single-pass `Reference::Index` applies to it. Fix the
   `calibrate_align_stack` doc to name it.
8. CAL-14: `CalibrationMasters::build(CalibrationSet<&[P]>, …)` is the only master builder, with
   a per-role cache hook for `lens`, and `lens` calls it. `stack_cfa_master` becomes private.

**Tests:** every pipeline, loader and drizzle test stays green with unchanged pins; a new
`align_and_stack_paths` test spills calibrated RGB FITS lights; `keep_cache` reuse on the RAM tier
and in a registered RAW run skips the decode (count decodes through a test hook); a RAW
`LinearImage` stack's peek carries the same `decoder_bytes` as the mosaic peek of that file; a
drizzle input with a non-finite sample fails admission with its index.

**Crates:** lumos, lens, darkroom.

---

## Batch 21: Platform entity, statics, disk space — low-medium

**Findings:** PIP-14, PIP-15, PIP-16, PIP-22, PIP-24.

**Problems.**
- **PIP-14.** Platform branches sit outside one entity: `frame_store/stored_plane.rs` (a
  `#[cfg(windows)]` field, `#[cfg(unix)]` advice), `frame_store/run_scratch.rs` (`open_deleting`
  per OS), `ingest/ingest_config.rs` (`default_cache_root` on `cfg!(windows)` and
  `cfg!(target_os = "macos")`), and `memory/mod.rs` (the macOS zero-available workaround inline).
  `memory/cgroup_memory.rs` and `mount_table.rs` are Linux-only and work because the files are
  absent elsewhere. Windows Job-object limits are never read, and no API says so.
- **PIP-15.** Mutable statics where an owner would do: `GROUPS` and `SYSTEM` in `memory/mod.rs`
  (the `SYSTEM` comment justifies a 15 µs saving, which the outside world does not force) and
  `COUNTER` in `frame_store/run_scratch.rs`.
- **PIP-16.** No disk-space check before a spill: a run finds `ENOSPC` after hours of decode and
  registration, and loses the work. Siril computes the output size and tests the free space first
  (`.tmp/siril/src/core/processing.c`, `io/sequence.c` `seq_compute_size`, `core/OS_utils.c`
  `test_available_space`).
- **PIP-22.** `frame_store/frame_spill.rs` names cache files with `FramePlane`'s `Display`, which
  is prose for errors: rewording an error renames the files, and the commit then misses them and
  leaves orphans.
- **PIP-24.** Free `pub(crate)` functions where an owner exists: `memory::{available_memory,
  memory_budget, load_concurrency}` (for `RunMemory`), `frame_spill::{write_file, map_file}`
  (only `StoredPlane` uses them), `pipeline/align.rs` `log_detection` (only `light_source` uses
  it). `stored_plane.rs` discards the `advise` result without saying why.
  `pipeline/detector_pool.rs`
  and `ingest/ingest_run.rs` are over the 40% test share.

**Design.**
1. One `platform/` module: `mod.rs` picks `linux.rs`, `macos.rs` or `windows.rs`, with Unix code
   both share in `unix.rs`. Its API is the same on every OS, and no caller uses `cfg`:
   - `ScratchFile`: create deleted-on-close, map, advise (a documented no-op where the OS has no
     advice);
   - `SystemMemory`: the available memory and the process's limit. Linux reads cgroups
     (`CgroupMemory` and `MountTable` move in), Windows reads the Job object, macOS has no limit
     source (a documented `None`). The macOS zero-available workaround moves into `macos.rs`;
   - the default cache root (`default_cache_root`);
   - the free space of the filesystem that holds a path.
2. Windows memory (decided: the `windows` crate 0.62, already in `Cargo.lock`, a Windows-only
   workspace dependency with `Win32_System_JobObjects`): available = the smallest of the free
   memory, the process limit less the process's committed memory, and the job limit less the
   job's committed memory, each limit only where its flag (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`,
   `JOB_OBJECT_LIMIT_JOB_MEMORY`) is set. The cgroup reader also takes a limit less its usage: a
   limit alone overstates what a process that already holds memory can allocate. Confirm the
   query for the job's current commit against the Windows SDK documentation before writing it.
   `common` keeps its `windows-sys` use as it is.
3. PIP-15: `SystemMemory` is created per run in `RunMemory::read`, with no `SYSTEM` or `GROUPS`
   statics. `COUNTER` moves onto `RunScratch`; `create_new` with `AlreadyExists` already handles
   collisions across instances.
4. PIP-16: where Batch 20 builds the `Tier`, before any write, estimate the bytes the run writes:
   the stored frames (warped, drizzled or decoded, as the plan sizes them), the parked set on the
   Auto path, and the kept-cache frames not yet committed when `keep_cache` is set. Compare with
   the free space of the filesystem of the scratch root, and of the kept cache when it is on
   another one. Free space comes from `sysinfo::Disks` (already a dependency): the disk whose mount
   point is the longest prefix of the canonicalized root. Fail with
   `FrameStoreError::InsufficientSpace { needed, available }`. The check is advisory: space can
   still run out later.
5. PIP-22: `FramePlane::file_tag()` names cache files; `Display` stays prose.
6. PIP-24: the three memory functions become `RunMemory` methods; `write_file` and `map_file`
   move into `stored_plane.rs`; `log_detection` moves to its user; the ignored `advise` result
   says why it is ignored (advice is a hint); split the two over-tested files.

**Tests:** the `SystemMemory` and `ScratchFile` APIs on Linux; `InsufficientSpace` from a fake
free-space figure through a test seam, with the needed bytes hand-computed for a spilled warped
run and a spilled drizzle run.

**Crates:** lumos.

---

## Batch 22: Combine hot loop — medium (measure first)

**Finding CMB-6.** The gather in `combine/cache/mod.rs` sets the worst-case row time for large
N. Per sample it tests the loop-invariant `Option`s (`frame_norms`, `weights`, `flags`, `noise`),
asks `gates[frame].sample`, and with noise on calls `noise.model(frame, slot)`, interpolates the
frame's flat gain (`rows.at(slot, gain_point)`), and divides `gain / (electrons · q)`. The
pixel-outer, frame-inner order touches one cache line in each of N image planes, plus quality and
flag planes, per 16 pixels; beyond about 32 streams the L2 prefetcher stops tracking them, so
spilled frames take demand misses. Siril has the same strided gather (`median_and_mean.c`), so
this is parity, not a regression.

**Steps:**
1. For each row tile of 64–256 px, loop the frames on the outside and write a `[tile][frame]`
   buffer of normalized values, mask bits, effective weights (the drizzle drop weight included)
   and noise terms. Then reduce each pixel from contiguous memory.
2. Apply `v · gain + offset` with `simd::Isa` as a multiply then an add, never a fused
   multiply-add, so it rounds as the scalar code does.
3. Monomorphize away the invariant `Option`s.
4. Keep `gain / (electrons · q)` a division: a precomputed reciprocal rounds differently. It
   vectorizes in the transposed pass.

**Acceptance:** `bench_stack_300` (large N and ragged rows) at least 1.3× faster; output bit
identical (the per-pixel f64 accumulation order is unchanged).

**Crates:** lumos.

---

## Batch 23: Defect map, repairs and bundle — medium (needs real data)

**Findings:** CAL-5 (decided: the map stays per master), CAL-16, and from Batch 26 CAL-10 and
CAL-15, which touch the same code.

**Problems.**
- **CAL-5, as decided.** A hot pixel is one whose residual over the per-colour dark background (a
  64 px colour mesh, confirmed against a robust plane through a ring of same-colour photosites
  6–8 px away) exceeds `DEFAULT_SIGMA_THRESHOLD = 5` times the master's robust σ
  (`detect_hot_pixels` in `calibration_masters/defect_map/mod.rs`). This stays: every reference
  builds the map from the master alone. Open is only its size: nothing reports the flagged
  fraction per colour, so the coverage loss CAL-5 described cannot be checked.
- **CAL-16.** `CfaLattice::median` (`io/image/cfa/cfa_lattice.rs`) repairs a Bayer green from
  its four diagonal greens at √2, which are the other phase (Gr against Gb). On a sensor with Gr/Gb
  imbalance every repair carries the other phase's level. The function serves defect repairs and
  null repairs.
- **CAL-10.** `calibration_masters/fits.rs` reads the whole bundle with `fs::read` (three f32
  masters of a 60 MP sensor are about 720 MB, beside the decoded masters), and every failure is
  `io::Error::new(InvalidData, String)`, not an enum of cases.
- **CAL-15.** `DefectMap` and its methods are `pub` (`lib.rs`) with no caller outside lumos:
  `lens` and `darkroom` use only `defect_summary()`, and `count()` has test callers only.

**Design.**
1. Each hot pixel keeps its excess `d_p`: its residual over the ring's plane, or over the mesh
   where the ring has none, the value the detector already tests. The bundle stores it beside the
   index (bundle version 5). Batch 27 fits the dark's scale on it.
2. `DefectSummary` adds the hot and cold counts per colour, so the flagged fraction per colour is
   reported, and `lens` shows it.
3. **CAL-16:** Bayer green repairs use same-phase greens only (Gr from Gr, Gb from Gb): the four at
   distance 2 and the four at 2√2. A same-phase median has no Gr/Gb bias by construction, so no
   measurement is needed to decide it. Every Bayer green repair through `CfaLattice::median`
   changes (defects and nulls); the pins move, and the report says so.
4. **CAL-10:** the bundle loads through a memory map and fails with `BundleError::{NotABundle,
   Version { found }, Checksum { hdu }, DuplicateExtension, …}`.
5. **CAL-15:** `DefectMap` and its methods become `pub(crate)`; fix the `crate::DefectMap` doc
   link in `io/image/cfa/mod.rs`; gate `count()` for tests.

**Tests:**
- The stored excess of each hot pixel equals its hand-computed ring residual, and the summary's
  counts per colour are exact on a synthetic dark.
- A Gr defect is repaired from Gr neighbours only, to the exact median.
- The bundle round-trips indices and excesses bit-exact; a version 4 bundle fails with
  `BundleError::Version { found: 4 }`.

**Crates:** lumos, lens, darkroom.

---

## Batch 24: Diagnostics and progress — low-medium

**Findings:** PIP-8, PIP-9 / CAL-13, PIP-26, RAW-18, PIP-20.

**Problems.**
- **PIP-8.** In a registered run only survivors reach `stack_stored_frames`, and combine errors
  index frames by position (`facts.admit(index, …)`, `NonPositiveMedian { index }`). Once a frame
  drops, an error names the wrong light. `AlignStackResult` uses input indices everywhere else.
- **PIP-9 / CAL-13.** `AlignStackError::Calibration(#[from] CalibrationError)` carries no path, so
  a temperature mismatch on one light of hundreds does not say which. `Load` and `CosmicRay` carry
  one.
- **PIP-26.** `lens/src/astro/nodes/stacking.rs` (twice) and `lens/src/astro/nodes/tests.rs` say
  "unset picks the richest frame", and the test `auto_reference_picks_the_richest_frame` agrees.
  `Auto` picks the lowest median FWHM among the frames with enough stars (`pipeline/config.rs`,
  `pipeline/align.rs`).
- **RAW-18.** `raw_files` (`io/raw/raw_files/mod.rs`) calls `fs::metadata` on every entry before
  the extension test and aborts on any one error: a dangling `notes.txt` symlink makes the
  directory unreadable as a light set. Its errors are message strings.
- **PIP-20.** `progress/stage_counter.rs` runs the user's progress callback under a `Mutex` on
  rayon workers. A callback that uses rayon can deadlock: a worker blocked in a `join` steals a job
  that calls `complete_one` on the same thread, and the lock is not reentrant.

**Design:** a stored frame carries its input index, set when it is parked or drizzled, and combine
errors report it. `AlignStackError::Calibration { path, source }` replaces the blanket `#[from]`.
`lens` describes `Auto` as "the frame with the lowest median FWHM among those with enough stars",
and the test is renamed to match. `raw_files` tests the extension first, uses
`entry.file_type()`, follows a link only for RAW-named entries, and returns a typed error that
names the entry. Progress counts with an atomic and reports in order through a sequencer that
never calls out while it holds a lock: the thread whose increment completes a contiguous run
reports it.

**Tests:** a registered run whose third input drops and whose fourth fails admission names input
index 3; a calibration error carries the light's path; a dangling `notes.txt` symlink does not
break a RAW scan, and a dangling `x.cr2` symlink is an error that names it; a progress callback
that calls `rayon::join` completes (a watchdog thread fails the test after 10 s instead of a hang).

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
1. **Pixels:** Batch 23's hot pixels not flagged in the light
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
| PIP-5 | A parked frame is warped straight from a map advised `Sequential` (`frame_store/stored_plane.rs`, `pipeline/pipeline_frame.rs`). A 180° flip reads rows in reverse, a 90° turn reads one column per output row, against the readahead. The plan already charges the source as resident, so the map saves nothing. | After Batch 21. Add legs to `pipeline/bench.rs`: Auto + spill warp at 0°, 90° and 180°, `Sequential` against `Normal`. Adopt `Normal` (or an owned read) if any angle is at least 1.2× faster and none is slower. |
| PIP-6 | Auto + spill parks the demosaiced frame (3 planes) where the calibrated mosaic (1 plane plus flags) would do, at the cost of a second demosaic. | Bench both on SSD; adopt the faster. The output is identical, because the demosaic is deterministic. |
| PIP-18 | `FrameStats::measure`'s `slot_median` collects each slot's samples, one frame across the slots in parallel: what `DECODE_TRANSIENT_FACTOR = 2` pays for, and it caps decode concurrency on tight budgets. | Replace the copy with the exact radix select (`math/statistics/radix_median.rs`) over the f32 bits in streaming passes with the same keep predicate. Adopt if bit identical; then set `DECODE_TRANSIENT_FACTOR` to the largest transient left. |
| PIP-19 | Disk I/O and compute do not overlap: each chunk boundary is a barrier and then a burst of page faults. DSS measured reads at about 7% of combine time. | After Batch 21. `madvise(WILLNEED)` the next chunk through `ScratchFile`; adopt if the spill-tier combine bench gains at least 10%. |
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
