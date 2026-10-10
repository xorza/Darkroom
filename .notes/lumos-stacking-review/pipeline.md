# Lumos stacking pipeline: architecture and data-plumbing review

Scope: `pipeline/`, `frame_store/`, `combine/cache/` (I/O side), `ingest/`, `memory/`, `concurrency/`,
`buffer_pool.rs`, `mount_table.rs`, `progress/`, `run_report.rs`, `error.rs`, `lib.rs`, and how
`lens` calls the stacking entries. Read-only review. No files in the repo were edited. Paths are
relative to `lumos/src/` unless noted otherwise.

## Summary

The pipeline holds up well on the basics. Spills are lossless f32. Scratch files are deleted
while still open, so cancel, panic and crash all clean up on their own. The tier decision is read
once per run. Results are deterministic, and RAM-tier and spill-tier runs are bit-identical.
Statistics are measured before the warp. The problems are in three places:

1. **The combine reads spilled data too often.** Channel-independent quality planes are re-read
   once per channel. On top of that, three extra full passes run after the warp: the
   common-domain build, normalization, and the coverage pass. An RGB warped spilled light is read
   about 18.75 plane-equivalents in the combine phase, against 5.25 needed (PIP-2, PIP-3, PIP-27).
2. **Ingestion is implemented twice.** `combine/cache/loader` and `pipeline/light_source` each
   implement peek, plan, admit, bounded decode and tier store. The tier decision has four
   representations, and frame carriers multiply. The `ingest` module holds only config and
   admission, and depends on `combine` in a cycle (PIP-11 to PIP-13).
3. **Gaps in the public API and the cache.** Already-calibrated lights have no tiered path from
   disk (PIP-10). `keep_cache` silently does nothing on the RAM tier and in registered runs (PIP-4).
   Kept-cache integrity rests on length and mtime only (PIP-17).

There are also platform `cfg`s outside one entity, mutable statics, error indices that count
survivors rather than inputs, and calibration errors without the light's path.

---

## Findings

### PIP-2 — The combine re-reads every frame's coverage, confidence and flags once per channel
- **Where:** `combine/cache/core.rs:119-120` (`for channel { for chunk { … } }`), `combine/cache/mod.rs:378-394` (`quality_chunks` and `flags` rebuilt inside the per-chunk closure), budget at `combine/cache/mod.rs:624-636`
- **Category:** performance (I/O)
- **Impact:** medium-high on the spill tier. For an RGB warped set, the 2 quality planes and the flag byte (9 B/px) are faulted in 3 times instead of once. That is 27 B/px against 12 B/px of channel data, so the combine moves about 1.8 times the bytes it needs. `MADV_SEQUENTIAL` (`frame_store/stored_plane.rs:58`) makes the second and third reads more likely to come from disk.
- **Confidence:** confirmed.
- **Evidence:** DSS writes band files that hold every channel of a row band for all frames and reads each band once (`.tmp/dss/DeepSkyStackerKernel/MultiBitmapProcess.cpp:55-93`, `:405-430`). Siril splits blocks per channel (`.tmp/siril/src/stacking/median_and_mean.c:296-360`), but it carries no per-frame quality planes.
- **Direction:** Iterate chunk-outer, channel-inner. Gather the quality and flag slices once per chunk and reduce all channels against them. Price `ChunkMemoryLayout::input_bytes` as `(C + quality.count()) * 4 + flags` per frame. The tier-equivalence test then guards the change.

### PIP-3 — Three extra full passes over the spilled set after the warp
- **Where:** `combine/normalization/common_domain.rs:41-63` (every coverage plane, single-threaded `for frame in frames`); `combine/normalization/mod.rs:401-440` (`measure_plane`, every channel plane); `:495-523` (`source_noise_variance` touches the confidence plane at 65 536 stratified indices, which is about every page); `combine/cache/mod.rs:255-302` (coverage pass in `finish_product` re-reads every coverage plane)
- **Category:** performance (I/O passes)
- **Impact:** medium-high on the spill tier. See the data-flow table. The coverage pass recomputes, with the same `PixelCoverage::contributes` gate, a count the combine already makes per pixel (`covered`, `mod.rs:475-511`). The `CommonDomain` build is serial I/O over N mapped files.
- **Confidence:** confirmed.
- **Direction:** Fill the coverage plane inside the combine on the first channel pass (`covered / N`). It is the same gate, so the result is bit-identical. Build `CommonDomain` in parallel (per-frame word masks, then AND-reduce in index order; AND is order-independent). Consider computing the domain mask during the warp, since the warp knows coverage per output pixel and could AND into a shared bitset. That would remove the coverage read entirely.

### PIP-4 — `keep_cache` only works on the disk tier, and registered runs never use it
- **Where:** `combine/cache/loader/mod.rs:121-125` (the RAM branch never opens the cache), `:300-303`; doc `ingest/ingest_config.rs:22-26` ("Keep each decoded frame in the decode cache, for a later run on the same files to reuse"); `pipeline/light_source.rs:382-406` (the RAW lights path never consults it)
- **Category:** design / correctness of documented behavior
- **Impact:** medium. A rerun that fits in RAM decodes every frame again even when a committed cache exists, and a `keep_cache` run that fits in RAM writes nothing. `calibrate_align_stack` and `align_and_stack` ignore the flag completely. So it serves only `stack()` of linear files and `stack_cfa_master` without a subtraction step, and only when they spill. The doc promises more. A re-decode costs the libraw unpack (the expensive part); mapping committed f32 does not.
- **Confidence:** confirmed.
- **Direction:** Make the kept cache a decode-level memo, independent of the tier. Look it up before any decode and map it (or read it into RAM on the RAM tier). For registered RAW runs, cache the uncalibrated `CfaImage` decode, which is what the key describes, before the master step. Otherwise narrow the doc and the field name to what it really does.

### PIP-5 — A parked frame is warped straight out of an `MADV_SEQUENTIAL` map
- **Where:** `frame_store/stored_plane.rs:54-58` (`Advice::Sequential` on every map), `pipeline/pipeline_frame.rs:24-35` (the warp source is the map, in place)
- **Category:** performance
- **Impact:** medium on the Auto + spill path. A 180° meridian flip reads source rows in reverse. A 90° camera rotation reads one source column per output row. Sequential advice makes the kernel read ahead in the wrong direction and stop marking pages as accessed, and the spill tier runs under memory pressure by definition. The plan already charges the source as resident (`PerFrameBytes::working`, `memory/memory_plan.rs:12-31`), so the mmap saves nothing.
- **Confidence:** likely (Linux `VM_SEQ_READ` semantics; not measured).
- **Direction:** Read the parked planes back with one sequential `read` into owned buffers before the warp; that is within budget. At the least, give warp-source maps `Normal`/`WillNeed` and keep `Sequential` for combine maps.

### PIP-6 — Auto + spill parks the demosaiced frame (3 planes) where the calibrated mosaic (1 plane) would do
- **Where:** `pipeline/light_source.rs:152-153` (`stage.tier.hold(image)` after the demosaic), `pipeline/frame_tier.rs:89-100`
- **Category:** performance (I/O against compute trade)
- **Impact:** medium, speculative. A Bayer light writes and reads back 12 B/px of demosaiced data. Parking the calibrated, cosmic-ray-cleaned CFA (4 B/px plus flags) cuts parked I/O by about two thirds. The price is a second demosaic in pass 2. Detection still needs the demosaiced frame in pass 1, and the statistics are already kept. DSS goes further and re-decodes everything in its stacking pass.
- **Confidence:** speculative. Measure demosaic time against disk bandwidth first.
- **Direction:** Benchmark parking the mosaic and demosaicing again in the registrar against the current parking, on SSD and on HDD. Keep whichever wins. The precision is identical, because the demosaic is deterministic.

### PIP-7 — An unwarped masked frame stores two identical f32 planes derived from a 1-byte mask
- **Where:** `frame_store/frame_quality.rs:98-108` (`confidence: coverage.clone()`), `frame_store/stored_image.rs:73-90` (`into_frame` reads the flags from the map, builds the validity plane, and writes both planes to disk)
- **Category:** performance / simplification
- **Impact:** low-medium. Every reference or decoded frame whose source has nulls carries 8 B/px of planes that hold 1 bit/px of information. On the spill tier they are written and then read once per channel in the combine (PIP-2).
- **Confidence:** confirmed.
- **Direction:** Add a `FrameQuality::Mask` (or derive coverage = confidence = valid from the flags plane already stored). Let the combine's gather read the flag byte. This keeps the pairing invariant by construction and drops two planes.

### PIP-8 — Combine-stage errors in a registered run index survivors, not inputs
- **Where:** `pipeline/registered_set.rs:44-69` (only survivors go to `stack_stored_frames`), `combine/cache/mod.rs:117-119` (`facts.admit(index, …)` over `frames.iter().enumerate()`), normalization errors (`NonPositiveMedian { index }`, `combine/normalization/mod.rs:243-270`)
- **Category:** correctness (diagnostics)
- **Impact:** medium. Once a frame drops, "frame 3 has a different row order/domain/CFA pattern" can name the wrong light. `AlignStackResult` documents input indices everywhere else (`pipeline/result.rs`).
- **Confidence:** confirmed.
- **Direction:** Carry the input index on `StoredFrame` (it is already known at park time), or pass an index map into `FrameCache::from_stored_frames`. Report errors through it.

### PIP-9 — Calibration errors in `calibrate_align_stack` lose the light's identity
- **Where:** `pipeline/light_source.rs:393` (`self.masters.calibrate(&mut cfa)?`), `pipeline/error.rs:40` (`Calibration(#[from] CalibrationError)`)
- **Category:** correctness (diagnostics)
- **Impact:** low-medium. "the dark's temperature 5 °C does not match the light's 0 °C" and "the light frame is already calibrated" arrive with no path or index. A load error carries the path; a calibration error does not.
- **Confidence:** confirmed.
- **Direction:** Use `AlignStackError::Calibration { path, source }`, mapped where `Load` already is. Drop the blanket `#[from]`.

### PIP-10 — Already-calibrated lights on disk have no tiered entry point
- **Where:** `pipeline/align.rs:42-55` (`align_and_stack(Vec<LinearImage>)`), `pipeline/calibrate.rs:39-69` (`calibrate_align_stack` loads only `CfaImage`), `io/image/cfa/mod.rs:254-265` (mosaic loaders only), doc at `pipeline/calibrate.rs:37-38` ("For frames that are already calibrated (e.g. pre-processed FITS), skip this and call `align_and_stack` directly")
- **Category:** design / public API
- **Impact:** medium. Calibrated RGB FITS lights (Siril or PixInsight output) have to be decoded in full by the caller before the run, so the inputs are resident and can never spill. The memory plan can only spill the warped set. The `LightSource` enum already has the right shape for a third source.
- **Confidence:** confirmed.
- **Direction:** Add a `LightSource::Decoded(&[P])` that runs `LinearImage::from_file` on a worker, plus a public `align_and_stack_paths`, or make `calibrate_align_stack` accept masters `None` with a decoder choice. Single-pass `Reference::Index` would then apply to it too.

### PIP-11 — Two ingestion implementations, and a cyclic `ingest` ↔ `combine` dependency
- **Where:** `combine/cache/loader/mod.rs` (465 lines: peek → `MemoryPlan::plan` → `FrameAdmission` → `try_par_map_limited` → RAM/scratch/kept store) against `pipeline/light_source.rs:101-197`, `:254-406` (peek → `MemoryPlan::plan|single_pass` → `FrameAdmission` → `DetectorPool::try_map` → `FrameTier`). `ingest/frame_admission.rs:7-9` imports `combine::cache::{frame_check, set_facts}` and `combine::error::StackError`, while `combine/cache/loader` imports `ingest::*`.
- **Category:** design / simplification
- **Impact:** medium. Each fix lands twice. PIP-4's cache exists only in one of them. Error and admission semantics have already diverged: loader frames get `SetFacts` in order at `from_tiered_paths`, pipeline frames only in the combine and indexed by survivor. The module named for the stage does not contain the stage.
- **Confidence:** confirmed.
- **Direction:** Move the decode-admit-store loop into `ingest/` as one type parameterised by the source (paths with an optional `FrameStep`, held images, RAW with masters) and by the sink (park or store). Both `FrameCache::from_paths` and `LightSource` consume it. Move `FrameCheck` and `SetFacts` into `ingest/`, and give admission its own error enum instead of `StackError`.

### PIP-12 — The tier decision has four representations
- **Where:** `memory/memory_plan.rs:84-89` (`fits_in_ram: bool`), `pipeline/frame_tier.rs:21-27` (`StagePlan`), `:53-63` (`FrameTier::{Ram, Spill{scratch, chunk_memory, parked}}`), `combine/cache/core.rs:38-56` (`CacheTier::{Resident, Spilled{chunk_memory}}` and `CacheTier::of(spilled: bool, memory)`), `combine/cache/loader/mod.rs:40-44` (`LoadedTier.spilled: bool`)
- **Category:** simplification
- **Impact:** low-medium. Each conversion is a place where `chunk_memory` can disagree. The bool-taking `CacheTier::of` is the strings-and-bools smell the user rules ask to avoid.
- **Confidence:** confirmed.
- **Direction:** Use one `Tier` enum (`Resident | Spilled { scratch: RunScratch, chunk_memory }`) built from the `MemoryPlan`, owned by `frame_store`. The combine reads `&Tier`. `parked` moves to the report counter that owns it.

### PIP-13 — Frame-carrier types multiply around one idea
- **Where:** `pipeline/detected_frame.rs` (`DetectedFrame{image, stars, diagnostics, stats}`) against `pipeline/frame_registrar.rs:47-53` (`FrameToPark{index, image, stars, stats}`), `light_source.rs:75-78` (`PassedLight`), `frame_registrar.rs:40-44` (`ParkedFrame`); `frame_store/stored_image.rs` (`StoredImage` = channels + flags + metadata) against `frame_store/stored_frame.rs` (`StoredFrame` = channels + quality + flags + stats); `light_source.rs:81-85` `Lights` duplicates `LightSource` with `Mutex<Option<_>>` cells, which is `concurrency::try_par_map_bounded_owned`'s idea; the loader's nine private types (`LoadedTier`, `LoadedCache`, `EarlyDecode`, `Decoded`, `CachedSource`, `CheckedImage`, `LoadedMemoryFrame`, `TierLoad`, `FrameDiskCache`)
- **Category:** simplification
- **Impact:** low-medium (readability and change cost).
- **Confidence:** confirmed.
- **Direction:**
  - `FrameToPark` becomes `&DetectedFrame` plus an index.
  - `StoredImage` becomes a `StoredFrame` with `FrameQuality::None`, with its metadata held beside it in `PipelineFrame::Spilled`. `into_frame` then shrinks to the quality derivation (or vanishes with PIP-7).
  - `Lights::Held` reuses the owned-cells helper.
  - Most of the loader types fold into the unified ingest of PIP-11.

### PIP-14 — Platform code is scattered rather than kept behind one entity
- **Where:** `frame_store/stored_plane.rs:27-28, 55-65` (`#[cfg(windows)]` field, `#[cfg(unix)]` advise); `frame_store/run_scratch.rs:68-93` (`open_deleting` per OS); `ingest/ingest_config.rs:57-68` (`cfg!(windows)` / `cfg!(target_os = "macos")` branches); `memory/mod.rs:48-53` (macOS zero-available workaround inline); `memory/cgroup_memory.rs` and `mount_table.rs` are Linux-only but work by "the file isn't there"; `io/raw/mod.rs:10,13,705,720`
- **Category:** style (user platform rule)
- **Impact:** medium (rule violation; Windows and macOS gaps are implicit). Windows Job-object limits are never read, and that gap is not part of any API.
- **Confidence:** confirmed.
- **Direction:**
  - `frame_store/platform/{unix,windows}.rs` behind one `ScratchFile` type: create deleted-on-close, map, advise.
  - `memory/platform/{linux,macos,windows}.rs` behind one `SystemMemory` type: available memory with cgroups or Job objects, documented as `Option` where a platform has no limit source.
  - The cache-dir default and the disk-space probe (PIP-16) go there too.

### PIP-15 — Mutable statics where an owner would do
- **Where:** `memory/mod.rs:32` (`static GROUPS: LazyLock<Option<CgroupMemory>>`), `:40` (`static SYSTEM: LazyLock<Mutex<System>>`), `frame_store/run_scratch.rs:53` (`static COUNTER: AtomicU64`)
- **Category:** style (user rule: no mutable statics unless the outside world forces it)
- **Impact:** low.
- **Confidence:** confirmed. The `SYSTEM` comment justifies a 15 µs saving, which is a convenience, not something the outside world forces.
- **Direction:** Read memory through a `SystemMemory` owner created per run in `RunMemory::read`, or once by the caller and passed in. Move `COUNTER` onto `RunScratch`; `create_new` + `AlreadyExists` already handles collisions across instances.

### PIP-16 — No disk-space check before a spill
- **Where:** `pipeline/frame_tier.rs:67-82` (`FrameTier::for_plan`), `combine/cache/loader/mod.rs:299-303`
- **Category:** correctness / robustness
- **Impact:** medium. A spill run discovers `ENOSPC` only after hours of decode and registration. The error propagates correctly, because writes go through `write()`, but all the work is lost.
- **Confidence:** confirmed.
- **Evidence:** Siril computes the output size and tests free space before it writes a sequence (`.tmp/siril/src/core/processing.c:181-182`, `io/sequence.c:781` `seq_compute_size`, `core/OS_utils.c:366` `test_available_space`).
- **Direction:** Estimate the bytes the tier will write (frames × `PerFrameBytes::warped`, plus the parked set on the Auto path) and check them against `statvfs`/`GetDiskFreeSpaceEx` through the platform entity. Fail with a typed `FrameStoreError::InsufficientSpace { needed, available }`. The check is advisory, since space can still run out later.

### PIP-17 — Kept-cache integrity rests on length + mtime, and one doc claim overstates it
- **Where:** `frame_store/frame_spill.rs:43-46` (doc: "a clash rebuilds the frame, it never reads another's"), `frame_store/cache_key.rs:110-116` (`CacheKey` has no source path), `common/src/file_utils/file_identity.rs:17-37` (`len` + `mtime_ns`), `PublicationMode::Cache` (no fsync, `common/src/file_utils/mod.rs:201-215`), reuse check `combine/cache/loader/mod.rs:432-446` (only finiteness and the quality pair)
- **Category:** correctness
- **Impact:** low-medium.
  - A same-size rewrite that keeps the mtime reuses stale planes silently. Examples are `rsync -t`, `cp -p`, or a re-export over the old file.
  - An FNV-64 stem collision with equal len and mtime reads another source's planes, so the doc's guarantee is false (the probability is tiny).
  - After a power loss, a commit can outlive its plane contents (XFS zero-filled extents), and zeros pass the finiteness check.
- **Confidence:** likely. The scenarios are real but rare.
- **Direction:** Record the canonical path and a per-plane blake3 digest in `Commit`. Verify the digest in the full-plane pass that reuse already makes (`stored_samples`); that is nearly free. Add the inode and ctime to the identity where the platform has them.

### PIP-18 — Statistics copy every channel, and the finiteness check is another full pass
- **Where:** `frame_store/frame_stats.rs:88-110` (`plane.to_vec()` per channel, in parallel = one full frame), `ingest/frame_admission.rs:53-58` (`FrameCheck::samples`, then `FrameStats::measure`), `memory/mod.rs:65` (`DECODE_TRANSIENT_FACTOR = 2` pays for the copy)
- **Category:** performance
- **Impact:** low-medium. The full-frame copy doubles each in-flight frame's decode peak, which caps `decode_concurrency` on tight budgets. The separate finiteness pass is one more memory-bound pass per frame.
- **Confidence:** confirmed.
- **Direction:** Fold the finiteness check into the copy. Better, compute the median and MAD exactly without a copy, using a radix/histogram select on the f32 bits in 2–3 streaming passes. That drops the transient factor to about 1 with bit-identical results.

### PIP-19 — Compute and I/O do not overlap
- **Where:** `frame_store/run_scratch.rs:40-48` (`write_all` of whole planes inside slot jobs on rayon workers), `combine/cache/core.rs:119-157` (no prefetch of the next chunk; pages fault inside the row loop)
- **Category:** performance
- **Impact:** low, speculative. On the spill tier, dirty-page throttling makes each writer block at disk speed. With `slots ≤ threads` blocked, cores idle. In the combine, every chunk boundary is a barrier followed by a burst of synchronous faults.
- **Evidence:** DSS measured file reads at about 7 % of combine time (`.tmp/dss/DeepSkyStackerKernel/MultiBitmapProcess.cpp:397-399`). Measure before investing.
- **Direction:** In the combine, `madvise(WILLNEED)` the next chunk's ranges, through the platform entity, before reducing the current one. On the write side, consider one writer thread with a bounded queue; it costs warp-buffer reuse unless double-buffered.

### PIP-20 — The progress callback runs under a `Mutex` on rayon workers
- **Where:** `progress/stage_counter.rs:36-45`
- **Category:** correctness hazard
- **Impact:** low. The callback is user code. If it touches rayon (or lumos), a worker blocked in a `join` can steal a job that calls `complete_one` on the same thread and deadlock on the non-reentrant `Mutex`. A slow callback also serializes every worker (this part is documented).
- **Confidence:** speculative (no in-tree callback does this; lens passes the default).
- **Direction:** Use an atomic counter, and deliver reports in order through a small sequencer that never calls out while holding a lock (for example, the thread whose increment completes a contiguous run reports it). Or document the restriction on `ProgressCallback::new`.

### PIP-22 — A human-readable `Display` string is used as an on-disk file name
- **Where:** `frame_store/frame_spill.rs:66-73` (`format_args!("_{plane}.bin")`), `frame_store/frame_quality.rs:46-53` (`Display` = prose for errors)
- **Category:** style (types over strings)
- **Impact:** low. Rewording an error message renames the cache files. The commit's `carries.quality` then finds them missing and silently rebuilds, leaving orphans.
- **Confidence:** confirmed.
- **Direction:** Add a `FramePlane::file_tag()`, as `DecoderKind::tag` does.

### PIP-24 — Smaller style deviations
- **Where:**
  - Free `pub(crate)` fns where an owner exists: `memory/mod.rs:27,60,69` (`available_memory`, `memory_budget`, `load_concurrency` → `RunMemory` methods); `frame_store/frame_spill.rs:211,221` (`write_file`, `map_file` are only `StoredPlane`'s helpers → move into `stored_plane.rs`); `pipeline/align.rs:59` (`log_detection`, used by `light_source`).
  - Test share above the 40 % rule: `pipeline/detector_pool.rs` (tests 86 of 164 lines), `ingest/ingest_run.rs` (gated code 41 of 76).
  - `mount_table.rs:20-23` keeps `filesystem` and `super_options` as `String` and compares them by string (acceptable as outside-format identifiers, but `is_memory_backed` could parse once).
  - `stored_plane.rs:58` `let _ = map.advise(...)` discards an error (fine for a hint; say so).
- **Category:** style
- **Impact:** low.
- **Confidence:** confirmed.
- **Direction:** Fold these into the batches that touch each file.

### PIP-26 — "Richest frame" wording does not match what `Auto` does
- **Where:** `lens/src/astro/nodes/stacking.rs:39,131` ("unset picks the richest frame"); test name `pipeline/tests/mod.rs:490` (`auto_reference_picks_the_richest_frame`); behavior `pipeline/config.rs:13-16`, `pipeline/align.rs:168-217` (lowest median FWHM among frames with enough stars)
- **Category:** docs
- **Impact:** low.
- **Confidence:** confirmed.
- **Direction:** Rename the test and fix the lens description; the lens part goes to that crate's issue log.

### PIP-27 — Normalizing an unwarped spilled set reads every page to gather 65 536 samples
- **Where:** `combine/normalization/mod.rs:401-440` (`measure_plane` walks `values.chunks` and samples at `k·n/m`), `stratified_indices` with no domain at `:451-455`
- **Category:** performance
- **Impact:** low-medium for `stack()` and master stacks on the spill tier. At 24 MP the stride is about 1.4 KB, under a page, so one extra full read of every plane happens just to pick 0.27 % of its samples.
- **Confidence:** confirmed.
- **Direction:** The no-domain index set depends only on `pixel_count`. Gather those samples in `FrameStats::measure` while the frame is in RAM, and keep them in the stats (and the sidecar). The read pass then disappears, with bit-identical samples.

---

## Data flow: every read, write and copy of one light frame

`P` is one f32 plane of W×H (4 B/px). `F` is a flag plane (1 B/px, only when the source carries
flags). The example is an RGB-from-Bayer RAW light.

### Front end (both references)

| # | Step | Where | Reads | Writes / copies |
|---|------|-------|-------|-----------------|
| 1 | RAW unpack → f32 CFA | `io/raw` via `CfaImage::from_file` (`light_source.rs:383`) | file (compressed) | 1P new |
| 2 | Master calibration in place | `CalibrationMasters::calibrate` (`light_source.rs:393`) | 1P + master planes | 1P in place |
| 3 | Cosmic rays (optional) | `reject_cosmic_rays` (`light_source.rs:394-401`) | ≥1P | in place + heap |
| 4 | Demosaic | `cfa.demosaic` (`light_source.rs:404`) | 1P | 3P new; CFA dropped |
| 5 | Finiteness check | `FrameCheck::samples` (`frame_admission.rs:53-57`) | 3P | — |
| 6 | Statistics | `FrameStats::measure` (`frame_stats.rs:88-110`) | 3P | 3P copy, then 2 select passes each in place; freed |
| 7 | Detection | `StarDetector::detect` | 3P → detection planes | about 9P of pooled scratch (kept per detector) |

### `Reference::Index` (single pass, `light_source.rs:254-379`)

| # | Step | Reads | Writes |
|---|------|-------|--------|
| 8 | Warp | 3P (+F) source | 3P + 2P quality (+F) into the worker's buffers |
| 9 RAM | Store | — | buffers move into `StoredFrame` (no copy) |
| 9 spill | Store (`frame_tier.rs:117-167`) | 3P + 2P (+F) | `write()` into unlinked files (copy into page cache → disk), then mmap; buffers reused |

**Writes to disk per frame (spill): 5P (+F).**

### `Reference::Auto` (`light_source.rs:101-164` → `align.rs:82-166`)

| # | Step | Reads | Writes |
|---|------|-------|--------|
| 7a spill | Park after detection (`frame_tier.rs:89-100`) | 3P (+F) | 3P (+F) to unlinked files |
| 8 | Warp from map (`pipeline_frame.rs:24-35`) | 3P from mmap (+F copied out) | 3P + 2P (+F) |
| 9 | Store (as above) | | 5P (+F) |

**Writes to disk per frame (spill): 8P (+2F). Reads back from disk: 3P before the combine.**
The reference that parked on disk is turned into a frame via `StoredImage::into_frame`: it reads F
and writes 2 identical P when nulls exist (PIP-7).

### Combine phase, per frame (warped set, `Normalization::Global`, coverage requested)

| # | Pass | Where | Reads |
|---|------|-------|-------|
| 10 | Debug-only contract check | `combine/cache/mod.rs:120-128` | everything (debug builds) |
| 11 | Common domain | `common_domain.rs:41-63` | 1P coverage (serial over frames) |
| 12 | Normalization medians + samples | `normalization/mod.rs:398-456` | 2 × 3P channels: the two radix passes of the common-domain median |
| 13 | Normalization noise | `normalization/mod.rs:495-523` | ≈1P confidence (stratified, about every page) |
| 14 | Combine | `core.rs:119-157`, `mod.rs:361-582` | per channel: 1P + 2P quality + F → **3 × 3.25P = 9.75P** for RGB (PIP-2) |
| 15 | Coverage pass | `mod.rs:255-302` | 1P coverage |

**Total combine-phase reads per RGB warped frame: about 18.75P; needed: 5.25P (3 channels + 2
quality + F).** For mono: about 8.25P against 3.25P.

### `stack()` / masters (`combine/cache/loader/mod.rs`)

| Step | Reads | Writes / copies |
|------|-------|-----------------|
| Decode (`I::load`) | file | C·P |
| Optional `FrameStep` (dark subtraction for flats) | C·P | in place |
| Admit (finiteness + statistics copy) | C·P twice | C·P copy |
| RAM tier | — | moved into `StoredFrame` |
| Spill tier, scratch | C·P (+F) | unlinked files |
| Spill tier, `keep_cache` | C·P (+F) | named temp + rename per plane, plus 2 sidecars |
| Kept-cache reuse | full read of every plane (finiteness + quality pair), then the combine reads it again | — |
| Combine | 1 read of C·P (no quality planes for unmasked files); normalization reads every page of every plane again (PIP-27) | — |

Nothing already in memory is re-read from disk on the warp paths: statistics are taken before
parking and the RAM tier never maps. The re-reads are all in the combine phase.

---

## Checked and found OK

- **Spill lifetime.** Scratch files are unlinked right after `create_new` (Unix) or opened
  `FILE_FLAG_DELETE_ON_CLOSE` (Windows), so disk space comes back on drop, cancel, panic or crash
  (`frame_store/run_scratch.rs:1-93`). Writes go through `write()`, not through the map, so
  `ENOSPC` surfaces as an error rather than a `SIGBUS`.
- **Spill precision.** f32 is written raw for channels and quality planes, with no f16 or
  quantization. This is better than Siril's ushort `r_` sequences when the input is 16-bit.
- **tmpfs/ramfs refusal.** `DiskRoot` (`frame_store/disk_root.rs`) uses mountinfo, longest mount
  point, last-listed shadowing, and octal unescaping (`mount_table.rs`). This was verified against
  the parser tests.
- **cgroup reading.** v1 and v2 both, the whole ancestor chain, the namespaced root stripped,
  `memory.high` treated as a cap, `inactive_file` counted as free, and the v1 unlimited sentinel
  handled (`memory/cgroup_memory.rs`). Swap limits are not consulted, which is conservative.
- **One memory reading per run.** `RunMemory` and `IngestRun` are passed everywhere. The 75 %
  budget is applied to the same raw figure at each site, not twice. `memory_override` stays apart
  from the decode ceiling.
- **`MemoryPlan` arithmetic.** It saturates throughout. The held-lights accounting nets out frames
  the caller already holds. Single-pass spill charges the decode peak, the detector and the warp
  buffers per worker.
- **Determinism.** RANSAC is seeded (`registration/ransac/sampling.rs:31`). Bounded maps splice
  results by index. The per-pixel combine is independent per pixel. Flag counts are integer
  atomics. `ram_and_streaming_tiers_produce_identical_stacks` and
  `two_default_runs_stack_bit_for_bit` pin it.
- **`try_par_map_bounded`.** The rolling window is one scoped task per slot. Failures return the
  lowest index, matching sequential semantics. There is no lock held across rayon calls (except
  PIP-20). Nested `par_iter` inside slot jobs is safe.
- **Cancellation.** It is polled per frame, per chunk and per row. The partial combine output is
  turned into `Cancelled` in `run_stacking`, and `park` returning `None` is safe because the token
  is monotonic within a run.
- **Reuse.** The reference is stored unwarped. Spill-tier warp buffers are reused across frames,
  and RAM-tier buffers become the stored frame with no copy.
- **Kept-cache commit protocol.** Planes go first, then the stats and commit sidecars. The layout
  tag is derived from a pinned digest, the decode version from characterization pins, and the
  options hash covers the checksum policy. The source identity is re-read after decode
  (`SourceChanged`). Cache-mode publishing is atomic (temp + rename).
- **Overflow.** Chunk sizing uses `checked_mul` and the plan uses saturating ops. Mapped-slice
  alignment is fine (page-aligned base, 4-byte offsets).
- **`Held` lights.** They are never spilled, so there is no pointless write and read-back.
- **Single-pass `Reference::Index`.** Each RAW light is written exactly once, matching or beating
  DSS, which decodes twice, and Siril, which writes registered files and then reads them.
- **Public surface.** `lib.rs` exports only through `pub use`. There are no in-crate re-exports and
  no production `super::` imports in the reviewed modules. `#[derive(Debug)]` is everywhere
  (`ProgressCallback` implements it by hand).

---

## Suggested batches

1. **Combine I/O passes:** PIP-2, PIP-3, PIP-27, PIP-7. All are bit-identical changes guarded by
   the tier-equivalence sweep. Expected effect: combine-phase spill reads go from about 18.75P to
   about 5.25P per RGB frame.
2. **One ingest stage:** PIP-11, PIP-12, PIP-13, then PIP-10 (a decoded-paths light source) and
   PIP-4 (the kept cache as a decode memo) on top.
3. **Platform entity and statics:** PIP-14, PIP-15, PIP-16 (the disk-space probe lives in the new
   platform module), PIP-22, PIP-24.
4. **Diagnostics:** PIP-8, PIP-9, PIP-26.
5. **Kept-cache integrity:** PIP-17. Do it after batch 2, since the cache moves there.
6. **Measure first (benchmarks before changes):** PIP-5, PIP-6, PIP-18, PIP-19, PIP-20.
