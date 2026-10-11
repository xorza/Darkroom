# Lumos open work — plan

Scope: the five open issues in `ISSUES.md` that touch `lumos` and `lens`, and what was left of
the stacking review (`lumos-stacking-review.md`, dropped here; its last text is in git at
`881ac82a5:.notes/lumos-stacking-review.md`). Paths are relative to `lumos/src/` unless stated
otherwise. Line numbers drift, so search for the named item.

## Summary

| ID | Item | Recommendation | Batch |
|---|---|---|---|
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
- **New dependencies**: none. `libc` is already a Linux dev-dependency of `lumos` (Q6).
- **Other platforms.** Only Linux builds here. Code for macOS and Windows is reported as not
  compiled unless the user runs it on the macOS test host or gives a Windows check.

## Dependency graph

```
B9                          independent
B6 → B8                     explicit spill reads → mosaic parking measured; both measure cold
                            reads with `StoredPlane::evict` (Linux; `libc` dev-dependency)
B7                          independent
```

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

**Measure (cold reads, `StoredPlane::evict` between passes).** Combine bench, spilled, 40 frames: map + faults against copy + overlap.
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

**Measure (cold reads, `StoredPlane::evict` between passes, after B6).** Pipeline bench, Auto + spill, both ways on SSD. Adopt the
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
