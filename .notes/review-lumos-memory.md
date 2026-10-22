# Review: lumos memory

Scope: memory use and memory management in `lumos/src`. Sizes are for a 24 MP frame
(6000 × 4000): an f32 plane is 96 MB, a u8 plane 24 MB, a bit plane 3 MB. No change below adds
disk I/O. Several remove reads or writes.

## Proposal

The model is sound: one `RunMemory` reading, one `Budget`, a `Stage` list priced by `Footprint`
chains, one `RunPlan`, one `StorageTier`. Hot loops in the combine, the drizzle kernels, the warp
rows, deblend and centroid allocate nothing per pixel, row or star. The defects come from five
root causes.

### Root causes

1. **Prices are hand-kept copies of the code they price.** Each `heap_bytes` is written apart
   from the allocations it counts. Unit tests pin the arithmetic, and `tests/memory_budget`
   measures whole runs from above only, against the budget, with no light a flat divided and no
   product save. So prices drift, and no test sees it: the drizzled frame leaves out its flat gain (7 GB unplanned on 100 frames at 96 MP),
   the gain warp's 72 MB point list and the product loader's second copy are not priced, the
   combine's cost has two formulas, deblend buffers are not charged, and `FrameStats` charges
   2 planes it never holds.
2. **Full planes for values read by row or at a point.** The sky σ planes, the matched filter's
   temp plane, the warp's f32 validity plane, the ML blend's four accumulators, the FITS writers'
   staged HDUs, the flag dilation's two scratch planes. Each holds a frame where a band or a
   point is read.
3. **Ownership forces a copy or a new allocation.** `StorageTier::store` consumes what it
   stores, so the drizzle builds a new output set for each frame, and the warp reaches around it
   with its own pool and `SpillFrame`. `Libraw` owns the file bytes through the whole decode. A
   peek drops what it read, so the first file is read or decoded again. A kept frame is copied
   into RAM and written again to the run scratch.
4. **One need, several mechanisms.** Per-worker state rides in bounded-map slots (detectors)
   and in a `ScratchPool` (warp results, with an unsafe `into_inner`). Per-job scratch is pooled
   in some places (mesh spline rows, combine) and allocated fresh in others (MRS tiles, à-trous
   rows, normalization samples). Two separable-filter designs exist. Four writers decide which
   planes a stored frame keeps, and they already disagree.
5. **Sizes from input reach an allocation unchecked.** A raster TIFF decodes with no limit, a
   drizzle scale has no upper bound, and the byte arithmetic of the prices can wrap.

### Target design

- **A price lives beside its allocation.** Each type that allocates for a frame owns the
  `heap_bytes` or `Footprint` of what it allocates. Stage builders only compose them. One
  constructor prices the combine for both the plan and the frame cache. Price arithmetic
  saturates, so a wrap can never make a run look small. Each run states the peak its plan
  expects, and `tests/memory_budget` holds a resident run's measured peak to it.
- **A plane exists only where a stage reads it as a plane.** A value read by row is computed by
  row into per-worker band scratch. A value read at a point is computed at the point. Results
  stay bit-identical: the lazy path runs the same kernel as the plane it replaces, and a test
  compares every pixel.
- **The store hands the buffers back.** `StorageTier::store` takes the frame by value and
  returns a `Stored { frame, returned }`. The resident tier moves the buffers into the frame and
  returns none. The spill tier writes them and returns them, so the producer (warp or drizzle)
  refills the same pages for its next frame. What a run reads once it keeps until its last use:
  the peek hands its bytes or its decode to the first load, and a kept frame is parked as the
  maps it already is.
- **One mechanism for each need.** State a worker carries between the items of a bounded map
  lives in its slot. Scratch inside a rayon job is a `ScratchPool` lease from `map_init` or
  `for_each_init` when the pool has an owner across calls. A call with no such owner allocates
  once for each worker (`for_each_init`), never once for each tile or job. One `StoredFrame`
  writer, over a plane sink (run scratch or decode cache), decides which planes a frame keeps.
- **Every size from input is bounded before it allocates.** The raster decoder and the drizzle
  grid check against the memory limit first, and pooled scratch that grows with input is charged
  or shrunk.

### Refused

- **A cosmic-ray detector kept per worker.** DESIGN.md refuses it.
- **Opening RAW files by path in LibRaw for the header peek.** It reverses the one-input-path
  decision and adds a platform split. The peek keeps its bytes for the load instead (batch 5).
- **A new heap meter, or `dhat`.** `tests/memory_budget` already counts the live heap
  (`live_heap.rs`) and runs each case alone in a child process (`alone.rs`), which the
  workspace's `cargo-test` runner needs. Batch 1 extends it.
- **Unit tests for each price, measured in the library's test binary.** Under `cargo-test`,
  other tests allocate on other threads of the same process during a measurement.
- **Moving the drizzle depth plane into the combine.** The peak does not change, and the shared
  combine takes on a drizzle concern.
- **Skipping the drizzle confidence plane when nothing reads it.** It helps one rare
  configuration and adds a mode.
- **A sparse index list for the `imposed` flags.** It needs two representations with a
  break-even switch. Only the duplicated builder is merged (batch 10).
- **A separable product weight, or one mono accumulator, in the ML blend.** Both change
  rounding. The band ring in batch 9 keeps the same sums in the same order.
- **Pricing the one-pass warp scratch beside `prepared.kept` only.** The worker keeps its
  `FlagScratch` from one light to the next, so the scratch is also held during the next decode.
  The current sum is correct.
- **A pool owner for the MRS tile scratch across calls.** No type spans the 2–4 calls for each
  light without a pool threaded through `FrameStats` and the detector. A scratch for each
  worker in each call removes nearly all of the allocations.
- **Reusing the reference's KdTree and triangles across registrations.** This saves CPU, not
  memory, and is out of scope.

## Plan

Each batch leaves `cargo drive check` and `cargo drive test` green. A batch that changes a
kernel or a hot loop (3, 4, 7) runs its benches before and after (`cargo drive bench <group>`),
and a slowdown is a defect of the batch. A batch that changes a vector kernel runs the inline
check in `lumos/AGENTS.md`. When a batch lands, its lasting decision goes into
`lumos/DESIGN.md` as a bullet, and its items are deleted here.

Batch 1, prices that match the allocations, landed. Dependencies: batch 6 needs the digest that
batch 5 computes at the write, and batch 7 needs the `Stored` return of batch 6. The other
batches are independent.

### 2. Streaming FITS output

The product save holds about 1.85 GB above the product at 24 MP RGB, at the end of a run that can
take hours. Files: `io/fits/product/mod.rs`, `io/fits/linear.rs`, `io/fits/cfa.rs`,
`io/fits/calibration_bundle/mod.rs`, `io/fits/flags_extension.rs`,
`io/fits/gain_extension.rs`.

- [ ] `io/fits/product/mod.rs:74-131`, `io/fits/linear.rs:56-58`, `io/fits/cfa.rs:112`,
  `io/fits/calibration_bundle/mod.rs:69-79`, `flags_extension.rs:48`, `gain_extension.rs:53` —
  each writer copies every HDU into an owned `Image`, and fits-well's `write_image` encodes each
  into a second full buffer. Target: one plane writer over `FitsWriter::stream_image`
  (`fits-well/src/fits_writer/mod.rs:291`) and `Writer::write_chunk`
  (`fits_writer/writer.rs:116`). It fills one reused `Data` chunk of a few MB band by band from
  the borrowed planes, and applies the NaN back-out, the mask bytes and the flag-bit stripping
  to each chunk, with no `BitBuffer2`. The three plane-concatenating encoders become this one
  writer. `Writer::finish` seeks back to patch `DATASUM` and `CHECKSUM`, and the file that
  `fs::publish` hands over can seek. A test compares the new file with the old one byte for
  byte.
- [ ] `tests/memory_budget/stack.rs` — no case saves a product. Target: a case that stacks, then
  saves the product, and asserts the save holds at most one chunk above the product.

### 3. Detection working set

Each detector slot prices 4 f32 planes plus a plane for the labeling's runs, and 4 bit planes,
and the plan charges every light in flight. This batch takes the f32 planes from 5 to 3, or 4 for
a demosaiced light (−96 to −192 MB for each slot). Files:
`star_detection/bg/{bg_estimate.rs, sky_noise.rs}`, `star_detection/refined_bg.rs`,
`star_detection/star_detector/stages/{prepared_frame/, detect/mod.rs}`,
`star_detection/convolution/`, `star_detection/threshold_mask/`, `star_detection/centroid/mod.rs`,
`star_detection/star_detector/resources.rs`, `star_detection/deblend/`,
`star_detection/labeling/labeler.rs`, `bg_mesh/mesh_spline/`, `math/wavelet/mod.rs`.

- [ ] `star_detection/bg/bg_estimate.rs:40` and `refined_bg.rs:45-65` — each refinement
  `estimate` interpolates a σ plane that `drop(sky)` frees unread. Target: `BgEstimate` holds
  the background only, and the σ comes from the mesh on demand.
- [ ] `star_detection/bg/sky_noise.rs:26` — `SkyNoise.noise` is a full plane. Its readers are
  `centroid/mod.rs:124` and `star_detector/stages/detect/mod.rs:166` (one pixel for each
  region) and `threshold_mask/threshold_params.rs:37-62` (row by row in `par_rows_mut`).
  Target: `SkyNoise` owns a copy of the mesh's tile statistics and the y spline's `d2y` (about
  100 KB), because later estimates overwrite the workspace mesh. `row_into(y, &mut [f32])` is
  `ColSpline::interpolate_row` (`bg_mesh/mesh_spline/mod.rs:158`) with the σ clip, into the
  row scratch of the threshold's workers. `at(pos)` runs the same row solve over the tile
  columns and the same interval kernel for one pixel. A test compares both with the plane bit
  for bit, the vector ISAs included. `BgEstimate::residual_into` and `subtract_from` read the
  background through the same rows. The fixtures that build a uniform σ plane
  (`internals/synthetic/bg_map.rs:20`, `detect/mod.rs:242`, `bg_estimate.rs:101-105`) build a
  mesh of one tile, whose rows are its one value.
- [ ] `star_detection/convolution/filter_kernels.rs:153-171` — the separable filter runs rows
  into a full `temp` plane, then columns back. `prepared_frame/mod.rs:167` copies `measure`
  into the plane first (96 MB, 2–3 times for each frame), and with no filter the copy is the
  plane. Target: one banded separable engine from a source plane into a separate output plane.
  Each band runs the row pass over rows `[start − r, end + r)`, with the column pass's edge rule
  at the frame's top and bottom, into band scratch, then the column pass into its output rows.
  The order of each sum stays, so the result stays bit-identical. It reads `measure` directly,
  and with no filter the stage reads `measure` itself. A filter cannot run in place in bands,
  because a band's row pass reads source rows that the band above writes.
- [ ] `math/wavelet/mod.rs:33` — `atrous_smooth` is a second separable-filter design, with
  `for_each_init(|| vec![0.0; width])` on each call (scales × channels for each op, and once
  for each MRS tile). Target: it runs on the banded engine, which takes the kernel's dilation
  step, with pooled row scratch.
- [ ] `star_detection/star_detector/stages/detect/mod.rs:147,184` — each pooled
  `DeblendBuffers` grows to the largest component it met (about 52 B for each component pixel,
  125 MB for a 2.4 MP nebula) and keeps that size for the run. `DetectionResources::heap_bytes`
  charges none of it. Target: `Pixel` with u32 coordinates (`deblend/pixel.rs:9-12`, 24 → 12 B),
  and a set over a stated bound shrinks before it returns to the pool. Charge the bound.
- [ ] `star_detection/labeling/labeler.rs:104` — the union-find is sized by foreground pixels
  (`count_ones`), not runs, and `LABEL_RUNS_PLANES` (`resources.rs:57-60`) leaves out the
  union-find and `mapping`. Target: size it by the run count, counted in the same pass, and
  charge the runs, the union-find and the mapping by run.
- [ ] `star_detection/star_detector/resources.rs:51-68` — set `PEAK_FLOATS` to the new peak,
  and pin it in `tests/pool_working_set.rs` and the detection case of `tests/memory_budget`.
  The bench at `threshold_mask/simd/bench.rs:29` reads a σ row instead of the plane.

### 4. Warp working set

Each warp in flight prices 8 B/px of `FlagScratch`. This batch takes it to 2 B/px (−144 MB for
each warp slot). Files: `registration/resample/{flagged_sources.rs, tap_window.rs,
frame_sampler/mod.rs}`, `image/pixel_flags.rs`, `simd/`, `image/cfa/cfa_image.rs`.

- [ ] `registration/resample/flagged_sources.rs:37,49,74` and `image/pixel_flags.rs:201` — the
  f32 validity plane is a 4 B/px copy of the flag bytes. It is read as a lane mask for values
  (`tap_window.rs:252-338`, `frame_sampler/mod.rs:166`) and as a ×1/×0 factor for weights.
  Target: build the lane mask from the flag bytes inside the windows that `near` marks. This
  needs a u8-load-to-mask op on `simd::Isa` for each ISA. A select of the weights gives the same
  sums with one exception: a Lanczos lobe is negative, so `0 × w` is `−0.0` where a select gives
  `+0.0`, and a window with every lane masked can sum to the other zero. Such a pixel has no
  coverage and is excluded. The cross-check test compares the outputs bit for bit where
  coverage is above zero, and by value elsewhere.
- [ ] `image/pixel_flags.rs:105-112,243` — the vertical van Herk pass holds two full u8 planes
  (48 MB), and `dilate_window` makes a new `DilationScratch` on each call (once for each
  demosaic, `cfa_image.rs:303`). Target: the vertical pass in bands of `reach.len()` rows, with
  O(block × width) scratch, and `DilationScratch::heap_bytes` to match. The existing dilation
  tests cover it, and a test compares it with the plane version over random flags.

### 5. Read each input once

Files: `pipeline/{light_files.rs, align_stack_run.rs, light_shape.rs}`, `io/raw/{mod.rs,
libraw.rs, unpacked_raw.rs}`, `io/load_ctx.rs`, `io/frame_info/mod.rs`, `frame/frame_peek.rs`,
`combine/frame_cache/loader/mod.rs`, `frame_store/{stored_plane.rs, run_storage.rs}`,
`frame_store/decode_cache/`.

- [ ] `pipeline/light_files.rs:74-77` — for a format with no header peek (RAW as linear,
  raster, a FITS whose `linear_info` fails), light 0 is decoded for its shape, dropped, and
  decoded again by `prepare(0)`. Target: the peek keeps the decoded light and hands it to the
  intake, as the combine loader's `Early` (`loader/mod.rs:117-122,168-170`) and the drizzle's
  `first` do. Price the held light in the shape.
- [ ] `frame_store/decode_cache/mod.rs:102-110` — `missing` resolves each path and reads,
  hashes and decodes each commit sidecar. Then each frame's `decode` or `reuse` does it all
  again. Target: one pass makes `Vec<Option<Committed>>` with the `CachedSource`s, and hands
  each entry to its decode.
- [ ] `io/raw/mod.rs:175-177` — `cfa_frame_info` reads the whole first file of each set (lights,
  and each master role through `D::peek` in `loader/mod.rs:117`) for its header, and the load
  reads it again. When the decode cache holds the file, the load never reads it, so the peek
  read it for nothing. Target: the commit records the `FrameInfo`, and the shape comes from file
  0's commit when the cache holds it. Otherwise the peek keeps the opened `Libraw`, and the
  load of file 0 takes it. Price the held file in the shape.
- [ ] `io/raw/libraw.rs:28` — `Libraw` holds the file bytes (25–60 MB) until it closes, while the
  f32 mosaic and the flags are allocated. Target: on `RawPath::Mosaic`, a `Libraw` method calls
  `libraw_recycle_datastream` after `unpack` and frees the bytes. After `unpack` the mosaic
  route reads only parsed state (`raw_image`, `data`, and the shim's `camera_clock`, which reads
  `other.timestamp`). `recycle_datastream` deletes only the stream object that
  `libraw_open_buffer` made (`LibRaw/src/utils/utils_libraw.cpp:282`), and `libraw_close` then
  finds no stream. The processed route keeps the bytes, because `dcraw_process` can read the
  file. The decode price (`RawPath::frame_bytes` beside `decoder_bytes`, and
  `FrameInfo::decoded`) becomes two steps: unpack holds the file and the raw u16 buffer, and the
  decode holds the raw buffer, the f32 mosaic and the flags.
- [ ] `frame_store/decode_cache/kept_frame.rs:126` with `frame_spill/mod.rs:219` — the digest
  of a kept plane is taken from the map that `StoredPlane::publish` (`stored_plane.rs:33-52`)
  just wrote. Under memory pressure this reads the pages back from disk. Target: `publish`
  hashes the slice it writes and returns the digest with the plane. `PlaneDigests::of` remains
  only for `reuse`.
- [ ] `frame_store/decode_cache/kept_frame.rs:165` and `mod.rs:91-92` — a reuse that goes
  into RAM hashes every map, then copies every map. Target: copy first, hash the copy, and
  compare.

### 6. One stored-frame writer; kept frames parked in place

Needs batch 5. Files: `frame_store/{stored_frame.rs, stored_image.rs, storage_tier.rs,
stored_gain.rs}`, `frame_store/decode_cache/kept_frame.rs`, `pipeline/{frame_parking.rs,
light_files.rs, align_stack_run.rs, pipeline_frame/}`, `image/flag_counts.rs`.

- [ ] `frame_store/stored_frame.rs:41-65,101-133`, `decode_cache/kept_frame.rs:78-128`,
  `stored_image.rs:98-122`, `pipeline/frame_parking.rs:70-101` — four writers decide which
  planes a frame keeps, and `FrameParking::store` repeats `StorageTier::store`. They already
  disagree: a reference parked on disk gets `samples: None` (`stored_image.rs:115`), but the
  same reference stored through `tier.store` (`frame_parking.rs:113-117`) writes 3 sample
  planes that no normalization of a registered set reads. Target: one `StoredFrame` writer over
  a plane sink (the run scratch, or the decode cache by name, which returns the digest of the
  write). `StorageTier::store` returns `Stored { frame, returned }` as the target design says.
  `KeptFrame::keep` becomes that writer with `FrameQuality::for_unwarped` and the commit, and
  `FrameParking::store` goes.
- [ ] `frame_store/stored_image.rs:35` with `decode_cache/kept_frame.rs:28,64,68-71,207` — the
  gain has two representations: `Option<Arc<FlatGain>>` on `StoredImage`, and its own
  `Option<StoredGain>` on `KeptFrame`, which `into_image` copies out of the maps
  (`stored_gain.rs:81-97`). Target: `StoredImage.flat_gain: Option<StoredGain>`, with no
  `KeptFrame` field.
- [ ] `pipeline/light_files.rs:123-124` with `pipeline/frame_parking.rs:52` — a linear light
  with `keep_cache` on the spill tier is read from the cache into RAM for detection, and then
  `StoredImage::spill` writes the same bytes again. That is 312 MB written for each RGB light.
  Target: `prepare` also returns the kept `StoredImage`, and parking takes it as
  `PipelineFrame::Spilled`. A mosaic light changes in its preparation and still spills.
- [ ] `frame_store/stored_image.rs:104` — `into_reference_frame` copies the whole mapped flag
  plane (`:85-93`, 24 MB) only to test `contains` and `keeps`. Target: keep the `FlagCounts`
  with the flag plane at the write, and read them here and in `into_image`.

### 7. Reuse across frames and jobs

Needs batch 6. Files: `drizzle/{accumulator/mod.rs, drizzle_run.rs, input_map.rs}`,
`concurrency/mod.rs`, `scratch_pool/mod.rs`, `pipeline/{frame_registrar.rs, frame_parking.rs,
align_stack_run.rs}`, `registration/resample/mod.rs`, `noise/mrs_noise.rs`,
`combine/normalization/{norm_fit.rs, photometric_gain.rs, slot_planes.rs}`,
`frame_store/stratified_samples.rs`, `combine/frame_cache/{loader/mod.rs, sample/mod.rs}`,
`combine/rejection/{scratch_buffers.rs, sorted_samples.rs}`.

- [ ] `drizzle/accumulator/mod.rs:121-145` with `drizzle_run.rs:228-238` — `drizzle` builds a
  new `OutputSums` for each frame, and on the spill tier the store writes it and frees it. That
  is C + 2W output planes for each frame (1.9 GB at 2× of 24 MP RGB) faulted in again each time.
  The warp recycles its output for this reason (`frame_parking.rs:66-69`). Target: the
  `Drizzler` keeps the `OutputSums` that the store returns and zeroes it in parallel before the
  next frame. The spilled plan already charges one frame in flight.
- [ ] `concurrency/mod.rs:145-148`, `pipeline/frame_registrar.rs:27,90,136`,
  `scratch_pool/mod.rs:182-195`, `pipeline/align_stack_run.rs:250` — the bounded map's doc says
  the warp keeps its output in the slot. The code passes `vec![(); warp_width]` and keeps the
  output in a `ScratchPool<WarpResult>`, whose only production `into_inner` caller is
  `frame_parking.rs:81`. Target: the slot holds `Option<WarpResult>` (with the detector on the
  one-pass route), and the store's `returned` refills it. Remove the pool,
  `impl Scratch for WarpResult` and the unsafe `into_inner`. The unit-slot idiom here and in
  `loader/mod.rs:172` becomes a width argument.
- [ ] `noise/mrs_noise.rs:288-335` — `tile_coefficients` allocates `pixels`, `smooth`, `next`
  and two f64 vectors for each tile (about 2.8 MB each, 80–320 allocations for each light).
  `heap_bytes` (`:183-197`) already prices one set for each worker. Target: one tile scratch
  for each worker in each call, from `for_each_init`, refilled for each tile.
- [ ] `combine/normalization/norm_fit.rs:357`, `photometric_gain.rs:224,293,343` — each
  (frame, slot) job copies a stored sample plane that `PairSide` only borrows, and allocates the
  `MedianMad` copy, `ratios` and `residuals` (the last one in each fit pass), 256 KB each.
  Target: `PlaneMeasurement.samples` borrows when the frame stores its samples, and one buffer
  set for each worker from `map_init`, beside `RadixMedian`, serves the rest.
- [ ] `frame_store/stratified_samples.rs:55-57` — `gather` returns a new `Vec` for each slot
  of each stored frame. Target: gather into a buffer the writer reuses.
- [ ] `drizzle/accumulator/mod.rs:206-226`, `drizzle/input_map.rs:108,131-137` — `bands`,
  the affine outline and the SIP outline are collected for each frame, and the SIP outline is
  used only for its row extent (`band_scan.rs:112`). Target: `bands` kept on `Drizzler` like
  `scans` (through `recycle::emptied`, as its elements borrow), the affine outline in an
  `ArrayVec<DVec2, 4>`, and the SIP outline folded straight to its row extent.
- [ ] `combine/frame_cache/sample/mod.rs:54`, `combine/rejection/scratch_buffers.rs:78-84`,
  `sorted_samples.rs:25-30` — `reserve(frame_count)` adds to the stale length that a lease
  carries, so capacities settle near 2 × `frame_count`. Target: clear first, then
  `reserve_exact(frame_count)`. Add the pooled `CombineScratch` (frames × 54 B plus
  `NormalScores` up to 1 MiB, for each worker) to the combine's `held`.

### 8. Raster decode bound

A bug: an input reaches an allocation that aborts the process. Files:
`imaginarium/src/image/io.rs`, `io/raster.rs`.

- [ ] `imaginarium/src/image/io.rs:86-97` and `io/raster.rs:30-41,74-78,136` — a raster TIFF
  decodes with `Limits::unlimited()`, and the `MAX_SIDE` check runs after the decode. A crafted
  header makes the decoder try to allocate up to 65535² × channels × 4 bytes. Target:
  imaginarium decodes with an admission callback, `Image::decode_admitted(reader, format,
  admit: impl FnOnce(&ImageDesc) -> Result<(), E>)`. It reads the header, calls `admit` with the
  description, and continues on the same decoder, so the file is opened and read once. lumos
  passes the side check and `OverBudget::check`. This is a submodule change, and imaginarium
  stays valid as a standalone checkout.

### 9. Cold image operations

Once for each master, but each holds hundreds of MB. Files: `image_ops/ml/{backend/mod.rs,
remove_stars.rs}`, `image/linear_image.rs`, `image_ops/{local_contrast, hdr}/mod.rs`,
`io/raster.rs`, `image/linear_pixels.rs`, `io/raw/unpacked_raw.rs`.

- [ ] `image_ops/ml/backend/mod.rs:99-118,210-275` — `FeatherBlend` holds 3 channel
  accumulators and a weight plane over the whole frame (384 MB), mono included. Tiles run in
  row-major order and tile starts are monotone, so the rows above the next tile row's start are
  final, and no later tile reads them. Target: accumulate into a ring band of `WINDOW` rows,
  with the same three channels and weight in the same order, and write each finished row's
  result in place. A mono frame keeps three band channels, so its mean is the same sum.
  `RemoveStars::split` (`remove_stars.rs:59`) clones the input so that the model can read the
  original while it writes. With a run that reads one image and writes another, `split`
  allocates the starless planes without the copy.
- [ ] `image/linear_image.rs:233-261` — `remap_intensity` clones channel 0 of a mono image
  (96 MB) to borrow it, and makes a new intensity plane for RGB. Target: borrow `channel(0)` for
  mono. For RGB, `local_contrast` and `hdr` compute `Rgb::intensity` where they read it, which
  gives the same bits.
- [ ] `io/raster.rs:107` — `LinearImage::from(&decoded)` copies even an `L_F32` image. Target:
  `From<Image>` by value, so a mono f32 plane moves.
- [ ] `io/raw/unpacked_raw.rs:476` — `vec![vec![0.0; n]; colors]` clones a zeroed plane before
  every sample is written. Target: build each plane directly.

### 10. Consolidation

Small items, each local. Files as named.

- [ ] `combine/frame_read/frame_chunk.rs:48-84` — `ChunkCopy` is `Vec<FrameBuffers>` of up to
  10 `Vec`s each, two copies (`core.rs:171-172`). Target: one `Vec<f32>` and one `Vec<u8>` for
  each `ChunkCopy`, with plane offsets computed in `new`. Readers fill disjoint `split_at_mut`
  runs.
- [ ] `combine/frame_cache/mod.rs:371-465`, `core.rs:119-184` — five near-identical blocks
  attach output row slices, and six `recycle::emptied(mem::take(..))` sites repeat one pattern.
  Target: one `ChunkViews` owner of the recycled vectors, with `begin` and `end`.
- [ ] `image/cfa/demosaic/bayer/rcd/tile.rs:132-155` — each RCD tile is about 33 `Vec`s.
  Target: one flat buffer with constant plane offsets. In `xtrans/markesteijn/mod.rs:100,144`,
  price the tile by `passes.directions()`.
- [ ] `calibration/defect_map/mod.rs:276` with `image/cfa/cfa_image.rs:246-248` — the
  `NO_DATA` mask is built four times for each light. Target: one predicate
  (`NO_DATA && !REPAIRED`) for `nulls`, and one mask that `calibrate` builds and shares.
- [ ] `calibration/calibration_masters/mod.rs:255-282` and `calibration/subtraction/mod.rs:143-160`
  — two builders of the `NO_DATA`-where-unmeasured plane. Target: one builder.
- [ ] `frame_store/stored_image.rs:52`, `decode_cache/kept_frame.rs:113`,
  `frame_spill/mod.rs:127-129`, `combine/frame_cache/loader/mod.rs:309,350,362`,
  `drizzle/drizzle_run.rs:208` — `ImageMetadata` is cloned where the owner can move it or the
  commit can serialize a borrow. In the drizzle it is built for each frame and kept only for
  frame 0.
- [ ] `pipeline/align_stack_run.rs:344-357` — the one-pass route maps `others: Vec<usize>`
  through `map_owned`, with a `Mutex` cell for each index, then inserts the reference. Target:
  `map` over an index range.
- [ ] `drizzle/drizzle_shape.rs:104-105` — the input footprint is built by hand. Target:
  `FramePeek::footprint(workers)`, as every other entry uses.
