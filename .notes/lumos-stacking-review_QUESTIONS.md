# Questions from executing `lumos-stacking-review.md`

Each entry names the plan item it blocks, the options, a recommendation, and what waits on the
answer. The plan keeps the blocked part with a pointer here.

## Q5. A kept cache for the registered pipeline and the drizzle (Batch 20, PIP-4)

**Finding.** `keep_cache` now serves `stack()` and the calibration masters on every tier. The
registered pipeline (`calibrate_align_stack`, `align_and_stack_paths`) and the drizzle entries
cannot use it as the cache stands: a kept frame holds its planes and its `FrameStats`, not its
`ImageMetadata`. Calibration reads a light's metadata (exposure, temperatures, sample domain,
calibration state, CFA pattern, white balance), the warp and the master carry it, and a RAW
light's is computed during its decode. The stack loader avoids the need by decoding frame 0 for
the stack's metadata and reading only planes and statistics for the rest; the pipeline needs every
light's metadata. `ImageMetadata` has no serde form, and one of its fields is `fits_well`'s
`SampleType`, which has none either.

| Option | What it does | Cost |
|---|---|---|
| **A. Persist the metadata** (recommended) | The commit records each frame's full `ImageMetadata` beside its statistics (serde on it and its parts; a mirror of `SampleType`; the sidecar pin moves). RAW runs cache the uncalibrated `CfaImage` decode, linear runs the linear one, and the pipeline reads a light back from the cache before any decode, on every tier | A serde form for about 40 fields and their types, kept in step with every new field; a kept RAW light costs its full f32 mosaic on disk, 4 B a photosite against the RAW's 1.5–2 |
| B. Re-read the metadata from the header | The cache keeps planes; a hit reopens the file for its metadata alone (a FITS header, a LibRaw open without unpack) | Metadata the decode computes (quantization σ, saturation, provenance) would have to come from the header path too, a second code path that must agree with the decode's, bit for bit |
| C. Leave it to `stack()` and the masters | The doc says so (it does now) | A rerun of a registered stack decodes every light again: for RAW, the LibRaw unpack, the slow part |

**Recommendation.** A: one source of truth (the metadata the decode produced), no second path to
keep in agreement, and the cost is code, not precision.

**Blocks.** Batch 20, PIP-4's registered and drizzle part. The `stack()`/master part is done.

## Q6. How to measure the spill tier's disk reads (PIP-5, PIP-6, PIP-19)

**Finding.** The three items change how the spill tier reads its maps: PIP-5 the advice a parked
frame is warped from, PIP-6 parking the mosaic instead of the demosaiced frame, PIP-19 a
`WILLNEED` on the next combine chunk. Each pays off only when the pages come from the disk, and on
this machine they never do: a run's scratch was written by the run itself and sits in the page
cache, with RAM to spare. Two ways to force cold reads were tried and failed:
- A user scope with `MemoryMax` below the spill (`systemd-run --user --scope`): the page cache
  is charged to the scope with the process's own memory, and the readahead of 40 sequential maps
  evicts the chunk being read. A 640 MiB combine read 1.77 TB in ten minutes at a 320 MiB cap and
  16 GB in 14 s at 480 MiB, a thrash, not a measurement.
- Evicting the planes between passes from inside the bench: memmap2 0.9 offers no `MADV_PAGEOUT`
  and no flush on a read-only map, and lumos has no `libc` to call `posix_fadvise` itself.

| Option | What it does | Cost |
|---|---|---|
| **A. Drop the caches between passes** (recommended) | A bench of one combine per process; a script runs `sync; echo 3 \| sudo tee /proc/sys/vm/drop_caches` before each, with and without the change | Root on the test machine, run by the user; no code beyond the bench |
| B. `libc` as a dev-dependency of lumos | The bench evicts its own planes (`posix_fadvise(DONTNEED)` after `fdatasync`) between passes | A new dependency, for the bench alone |
| C. A spill larger than RAM | Frames whose planes exceed the machine's memory, so each pass reads the disk | 30+ GB written per run here, slow, and wear on the SSD |

**Recommendation.** A: the measurement is the one the items ask for, cold reads, with nothing
added to the build.

**Blocks.** PIP-5, PIP-6 and PIP-19 in "Measure before you change".
