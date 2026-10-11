# Lumos open work — plan

Scope: the five open issues in `ISSUES.md` that touch `lumos` and `lens`, and what was left of
the stacking review (`lumos-stacking-review.md`, dropped here; its last text is in git at
`881ac82a5:.notes/lumos-stacking-review.md`). Paths are relative to `lumos/src/` unless stated
otherwise. Line numbers drift, so search for the named item.

## Summary

| ID | Item | Recommendation | Batch |
|---|---|---|---|
| I1 | FITS real keywords accept any value | Do | B1 |
| I2 | MRS noise reads 0 when the support covers every pixel | Do | B2 |
| I5 | Master cache key ignores how lumos stacks | Do | B3 |
| I3 | Pairwise CFA-drizzle normalization fits thin pairs | Do | B4 |
| Q6 | Cold-read eviction for the spill benches | Do (prerequisite) | B5 |
| I4, PIP-19, PIP-5 | Spilled reads fault maps the budget never counts | Do | B6 |
| PIP-4 | Kept cache for the registered pipeline and the drizzle | Do | B7 |
| PIP-6 | Park the mosaic instead of the demosaiced frame | Measure | B8 |
| — | First float-TIFF light decoded twice | Do, low priority | B9 |
| — | X-Trans cosmic-ray statistic | Defer | — |
| RAW-10, DMS-6 | RAW buffer vs file open; WB inside the demosaic | Blocked on real data | — |

I4, PIP-5 and PIP-19 are one defect class: the frame store reads spilled planes by faulting
memory maps, so the page cache the read needs is outside every figure the memory plan holds. B6
replaces the faults with explicit copies into buffers the plan counts, which fixes the thrash and
gives PIP-19 its overlap.

## How to run this plan

- **One batch per change.** Start a batch only when the tree is clean (the previous batch is
  committed). The user commits; do not commit.
- **Order.** Follow the dependency graph. Inside a batch, follow its steps in order.
- **Verification after each batch**, from `AGENTS.md`, for each crate the batch names:
  `cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings &&
  RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps --document-private-items
  --all-features && cargo test -p <crate> --tests --all-features`, with `--features ml` in place
  of `--all-features` in `cargo test` for `lumos`. A batch that changes a public `lumos` API also
  runs `lens` and `darkroom`. A change inside a submodule (`imaginarium`) runs its own chain from
  its own directory and stays valid in a standalone checkout.
- **SIMD kernels**: after a change to one,
  `cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)"
  $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter` must print 0.
- **Snapshots.** `internals/characterization` pins outputs. A batch that moves an output on purpose
  updates the pin and states why. An unexplained move is a regression.
- **Measure-first items** adopt a change only when the bench shows the stated gain and the output
  is bit identical.
- **New dependencies** are only `libc` as a `lumos` dev-dependency (Q6, approved).
- **Other platforms.** Only Linux builds here. Code for macOS and Windows is reported as not
  compiled unless the user runs it on the macOS test host or gives a Windows check.

## Dependency graph

```
B1, B2, B3, B4, B9          independent
B5 → B6 → B8                eviction helper → explicit spill reads → mosaic parking measured
B7                          independent; after B3 if both are open, as both move sidecar pins
```

---

## B1. FITS real keywords hold their physical ranges (I1)

**Evidence.** `io/image/fits/metadata/mod.rs`, `read_metadata`: `GAIN`, `EGAIN`, `SET-TEMP`,
`FOCALLEN`, `AIRMASS`, `XPIXSZ`/`YPIXSZ` and their aliases, and `DATAMAX` go through
`Header::get_real` with no check, so a negative gain, a 0 pixel size or an infinite value enters
`ImageMetadata` and is written back into every FITS lumos saves. Exposure and the temperatures
already use `read_real_where`. `XBINNING`/`YBINNING` take any `i32` and `ISOSPEED` takes 0. Only
`CcdNoise::electrons_per_unit` filters `egain` on its own; every other value travels as given.

**Solution.** The range is the field's property, so it lives on `MetadataField`:
`MetadataField::accepts(self, value: f64) -> bool`, one arm per real field, and every real field
reads through `read_real_where(header, key, |v| field.accepts(v))`. A value outside its range is
refused for that keyword, logged, and the next alias is tried, as a value of the wrong type is
now.

| Field | Accepted | Why |
|---|---|---|
| `GAIN` | finite, ≥ 0 | a camera's gain setting; 0 is a real setting |
| `EGAIN`, `CVF` | finite, > 0 | electrons per ADU |
| `SET-TEMP` | finite, > −273.15 | °C, as the measured temperatures |
| `FOCALLEN` and aliases | finite, > 0 | mm |
| `AIRMASS` | finite, ≥ 1 | sec z, which is 1 at the zenith and grows toward the horizon |
| pixel sizes | finite, > 0 | µm |
| `DATAMAX` | finite | file units, any sign |
| `XBINNING`, `YBINNING` | ≥ 1 | integer binning |
| `ISOSPEED` | ≥ 1 | |

**Steps.** 1. Add `accepts` and route the real fields through it. 2. Give the binnings and ISO
their bounds in `read_i32`/`read_u32` callers. 3. Keep `CcdNoise`'s own filter: `ImageMetadata`
is public and a caller can still set any value.

**Test.** Extend the field sweep in `fits/metadata/tests.rs`: for each field, the boundary that is
kept (`GAIN` 0, `AIRMASS` 1.0) reads back, and the values just outside (`EGAIN` 0, `AIRMASS` 0.99,
a pixel size 0, `−1`, `1e400` as written text) read `None`; an invalid first alias falls through
to a valid second one (`FOCALLEN = -5`, `FOCAL = 400` → 400).

**Crates:** lumos.

---

## B2. MRS noise falls back to k-σ when its noise set is too small (I2)

**Evidence.** `math/noise/mrs_noise.rs`, `sigma`: each iteration takes the standard deviation of
the pixels with `significance < K·σ`. When every analysed pixel is in the support — a small frame
whose coarse layers reach every pixel from one bright structure — that set is empty,
`standard_deviation` returns 0, and the loop ends at 0. A 48×48 frame of white noise crossed by one
bright 32-pixel line reads 0, and noise weighting then refuses it with `NoNoiseToWeigh`. The same
`sigma` serves `estimate_split`, per gain bin.

**Practice.** PixInsight's noise evaluation (`NoiseEvaluation`, as ImageIntegration uses it)
accepts an MRS estimate only when the noise set holds at least 1% of the pixels and the iteration
converged; otherwise it uses the k-σ estimate.

**Solution.** `sigma` returns the MRS figure only when the final noise set holds at least 1% of the
analysed pixels, and at least 2, and the iteration converged within `MAX_ITERATIONS`. Otherwise it
returns the k-σ estimate of the first layer, `k_sigma / LAYER_SIGMA[0] / K_SIGMA_RESPONSE`, with
the k-σ iteration run to `CONVERGENCE` (10⁻⁴) rather than `START_CONVERGENCE`, so it sits at its
fixed point. `K_SIGMA_RESPONSE = 0.98485`: an iterated 3σ clip of a Gaussian settles where
`s = √(1 − 2cφ(c)/(2Φ(c) − 1))` with `c = 3s`, solved numerically; derived, not measured, and the
test cross-checks it. The fallback is logged at debug with the noise-set fraction.

**Test.** In `math/noise/tests.rs`: the issue's 48×48 frame reads its σ within the k-σ estimator's
standard error (≈ 1/√(2·2304) ≈ 1.5%, so within 5%); `K_SIGMA_RESPONSE` against a Monte-Carlo
clip of 10⁶ Gaussian samples within 3·10⁻⁴; a frame where MRS converges reads exactly what it reads
now (`frame_stats` pin unmoved; if a fixture now takes the fallback, the pin moves with that
reason).

**Crates:** lumos.

---

## B3. A master is keyed by how lumos stacks it (I5)

**Evidence.** `lens/src/astro/nodes/calibration.rs`, `SourceDirectoryCache`, keys a master by
`MasterInputs`: its frames' names and `FileIdentity`, and the subtracted master's frames. Nothing
in the key moves when lumos's decode, its decode options (`IngestConfig::fits`), or the master
stack itself (`stack_cfa_master`, the role presets, `MasterSubtraction`, the null repair) change.
A master built by an older lumos is reused as current. The kept frame cache's `CacheKey` carries
the decode version and options; no pin covers the master stack — `calibrate_snapshot` starts
from masters given as images.

**Solution.**
1. `internals/characterization`: a `master_snapshot` that stacks a bias, a dark and a flat with
   the bias taken from each frame, through `stack_cfa_master` on mosaic FITS frames, and pins
   their planes and flags as `MASTER_PIN`.
2. `frame_store/cache_key.rs` (or a `master_recipe.rs` beside `master_cache.rs`):
   `MASTER_VERSION = pins_fingerprint(&[every DECODE_PINS entry, MASTER_PIN])`, derived like
   `DECODE_VERSION`.
3. `MasterInputs` gains `recipe: u64`: FNV-1a over `MASTER_VERSION`, the decode options of the
   ingest's `LoadContext` (the bytes `CacheKey::new` already hashes, factored into one function
   both call) and the role. `CalibrationMasters::build` fills it; its doc says a cache keys by it.
4. `lens`: `CachePlace::of` appends `recipe {:016x}` to the key.

**Test.** lumos: two FITS option sets give two recipes and two roles give two recipes (A → X,
B → Y, `X != Y`); the recipe is stable across two builds. lens: a key changes with the recipe
alone, the frames unchanged.

**Crates:** lumos, lens, darkroom.

---

## B4. Pairwise normalization only where it is the more precise estimator (I3)

**Evidence.** `combine/normalization/mod.rs`, `pairwise_norms`: a CFA-drizzle pair is fitted over
whatever its slot's `CommonDomain` holds. One shared pixel is fitted like ten thousand; only an
empty domain falls back to `per_frame_norms`. The rule decides by "any pixel" where it should
decide by which estimator gives the smaller error.

**Solution.** Decide for the whole set, as now, by predicted standard error:
- **Pairwise, `Global`:** the Deming slope's asymptotic variance over the inliers, from the
  paired moments and the two noise variances `PairedMoments` already holds (Fuller, *Measurement
  Error Models*, 1987, §1.3), returned beside the gain by `paired_photometric_gain`.
- **Pairwise, `Multiplicative`:** the median ratio's error from the two medians' errors over the
  `n` shared samples, `1.2533·σ/√n` each, in relative terms.
- **Per frame:** `Global` from `spread_variance_error` carried through `√(spread² − σ²)` and the
  ratio; `Multiplicative` from each frame's medians over all its samples.
- **Rule:** pairwise for the set when every pair-slot's relative gain error is no larger than the
  per-frame estimator's for that frame and slot; per frame for every frame otherwise. An empty
  domain is an infinite pairwise error, so today's fallback is one case of the rule. The log
  states the estimator and the worst pair's two errors.

**Test.** A set whose pairs share 30 pixels per slot takes per-frame; one whose pairs share
thousands takes pairwise. The Deming error cross-checks against the empirical spread of fitted
gains over 200 synthetic pairs within 10%; the test stays under 1 s. The issue's entry is removed
from `ISSUES.md`.

**Crates:** lumos.

---

## B5. Cold reads for the spill benches (Q6, prerequisite of B6 and B8)

**Decision (Q6, option B):** `libc` as a `lumos` dev-dependency, for the benches alone.

**Solution.** A bench helper, Linux-only behind the platform module, evicts a stored plane's pages
so the next pass reads the disk:
- a scratch plane has no name and, on Unix, no descriptor once mapped: `msync(MS_SYNC)` then
  `madvise(MADV_PAGEOUT)` (Linux 5.4+) on its map;
- a named plane (a kept frame): open it, `fdatasync`, `posix_fadvise(POSIX_FADV_DONTNEED)`.

`posix_fadvise` needs a descriptor the scratch plane no longer has, and keeping one per plane
would spend one descriptor per plane against the 1024 soft limit; `MADV_PAGEOUT` reaches the same
pages through the map.

**Test.** After an eviction, `mincore` reports no resident page of the plane; after a read, every
page.

**Crates:** lumos (dev-dependency, `bench` feature code).

---

## B6. Spilled planes are copied into counted buffers, not faulted (I4, PIP-19, PIP-5)

**Evidence.**
- I4: `combine/cache/core.rs`, `process_chunks`, hands each frame's chunk to the combine as a
  slice of its map (`StoredPlane::chunk`). The chunk budget (`ChunkMemoryLayout`) counts those
  bytes, but they are page cache, charged to the cgroup outside the process's own memory and
  evicted at the kernel's choice. 40 spilled 2048² frames, `memory_override` 128 MiB, under
  `MemoryMax=320M` read 1.77 TB in ten minutes without finishing a pass.
- PIP-19: each chunk boundary is a barrier followed by a burst of page faults; I/O and compute do
  not overlap.
- PIP-5: `pipeline/pipeline_frame.rs` warps a parked frame straight from its map, advised
  `Sequential`; a 90° turn reads one column per output row. The plan already charges the source
  as resident, so the map saves no memory, and under a cgroup it has I4's thrash too.

**Solution.**
1. `StoredPlane::read_into(start, end, &mut [T])`: a resident plane copies its slice, a mapped one
   copies from its map front to back. The copy touches each page once in file order, so the
   page cache it needs is the readahead window, not the chunk.
2. `process_chunks` keeps two chunk buffers (`ChunkBuffers`, owned by the pass and reused): a
   reader thread fills chunk k+1 while the combine runs on chunk k. On the resident tier the
   chunks stay borrowed slices, no copy.
3. `ChunkMemoryLayout` charges `2 × input_bytes` per row on the spilled tier, so the two buffers
   fit the budget the plan already reads.
4. The warp of a parked frame copies the frame into an owned `LinearImage` first (one sequential
   read), then warps from RAM.
5. `MappedFile` keeps `Sequential`: every remaining map read is front to back.

**Measure (cold, B5).** Combine bench, spilled, 40 frames: map + faults against copy + overlap.
Adopt when the copy is no slower; report the overlap's gain (PIP-19 expected ≥ 10%). Warp bench,
parked, at 0°, 90° and 180°: owned read against the map.

**Test.** `disk_tier_output_is_bit_identical_to_memory_tier` holds; a pass over a spilled set
reads each plane once per chunk (count `read_into` calls through a test hook); the issue's cgroup
run finishes with total reads within 2× the spill size per pass (`/proc/self/io` `read_bytes`),
run by hand as a check left for the user if no cgroup is available to the test. The I4 entry is
removed from `ISSUES.md`.

**Crates:** lumos.

---

## B7. Kept cache for the registered pipeline and the drizzle (PIP-4)

**Evidence.** `IngestConfig::keep_cache` serves `stack()` and the calibration masters only.
`calibrate_align_stack`, `align_and_stack_paths`, `drizzle_stack` and `drizzle_cfa_stack` decode
every light on every run: a kept frame holds its planes and its `FrameStats`, not its
`ImageMetadata`, which calibration, the warp and the master all read.

**Decision (Q5, option A):** the commit records the full `ImageMetadata`.

**Solution.**
1. serde on `ImageMetadata` and its parts that lack it: `ImageProvenance`, `MosaicNoise`,
   `CalibrationState`, `FlatGain` (held by `Arc`), and `SampleTypeRecord`, a lumos mirror of
   `fits_well::SampleType` with `From` both ways, since the `fits-well` submodule stays
   serde-free.
2. `frame_spill.rs`: the `Commit` record carries the metadata; `SIDECAR_PIN` moves.
3. The kept-frame logic leaves `combine/cache/loader/mod.rs` (`CachedSource`, `kept_frame`, the
   before-and-after identity check) for one type in `frame_store` that every entry uses: the stack
   loader, `LightFiles::prepare`, and the drizzle file entries.
4. RAW lights keep the uncalibrated `CfaImage` decode, linear lights the linear one. A hit is
   copied into an owned image before calibration, which writes in place.
5. `IngestConfig::keep_cache`'s doc names every entry.

**Test.** A rerun of a registered stack and of a CFA drizzle with `keep_cache` decodes no light
(count decodes through a test hook) and gives a bit-identical output; a light rewritten between
runs decodes again; a metadata round trip with every field set compares equal.

**Crates:** lumos, lens, darkroom.

---

## B8. Park the calibrated mosaic, not the demosaiced frame (PIP-6, measure)

**Evidence.** Auto reference + spill parks each demosaiced light, three planes and flags, where
the calibrated mosaic, one plane and flags, would do, at the cost of a second demosaic.

**Measure (cold, B5, after B6).** Pipeline bench, Auto + spill, both ways on SSD. Adopt the
faster. The output is bit identical either way, since the demosaic is deterministic; a test holds
that.

**Crates:** lumos.

---

## B9. A float TIFF's dimensions from its header (low priority)

**Evidence.** `pipeline/light_source.rs`, `LightFiles::shape`: `LinearImage::peek` reads no TIFF
header, so the first float-TIFF light is decoded in full to size the run, before the plan exists,
and decoded again by the intake.

**Solution.** `LinearImage::peek` for TIFF through a header-only probe in `imaginarium`, a
matching submodule change. Where the probe cannot answer, keep today's decode.

**Test.** Peek gives the decode's dimensions on mono and RGB float TIFFs; the pipeline decodes
the first TIFF light once (decode counter).

**Crates:** imaginarium (own chain), lumos.

---

## Deferred

- **X-Trans cosmic-ray statistic** (research). A statistic that is zero on a linear slope,
  negative on a star's convex wing and positive at a peak, for example the residual of a robust
  local plane through the same-colour neighbours; `objlim` calibrated against mono L.A.Cosmic on
  Gaussian stars (σ 0.8–4 px, nine sub-pixel positions, every phase), with no star pixel flagged
  at the default and the mono path's false-positive rate on pure noise. No reference tool exists to
  check against, and X-Trans lights are refused cleanly now, so it waits for a real X-Trans set.
- **RAW-10**: `bench_unpack_file_vs_buffer` (`bench` + `real-data`) on CR2, CR3, NEF and RAF.
  Needs `test_data/lumos_data`.
- **DMS-6**: per-colour background gains used only inside the demosaic for OSC FITS without white
  balance; compare star colour and zipper artifacts with and without, adopt only if it helps. Needs
  real OSC data.

## Checks left for the user

- `DECODE_PINS.raw_cfa` is a placeholder: run `raw_decode_snapshot` with `real-data` and pin the
  digest it prints.
- The flagged fraction per colour on a cooled-CMOS master and on a DSLR set; if it is large on a
  well-stacked cooled master, the threshold multiplier is the lever.
- The dark-scale factor `k` across a DSLR session, and the residual hot-pixel pattern with and
  without it.
- `common/src/file_utils/platform/windows.rs` is checked only by a Windows clippy: run the `common`
  tests on Windows, `FileIdentity::of` above all.
- `lumos/src/platform/macos.rs` is not compiled here and `platform/windows.rs` only through a
  Windows clippy: build and test lumos on macOS and Windows, the free space and the Job-object
  memory above all.
