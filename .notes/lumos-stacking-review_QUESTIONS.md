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
