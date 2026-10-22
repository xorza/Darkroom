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

Batches 1, prices that match the allocations, 2, streaming FITS output, 3, the detection working
set, and 4, the warp working set, landed. Dependencies: batch 6 needs the digest that batch 5 computes at the write, and batch 7
needs the `Stored` return of batch 6. The other batches are independent.

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
`combine/rejection/{scratch_buffers.rs, sorted_samples.rs}`, `image/{pixel_flags.rs,
cfa/cfa_image.rs}`, `io/{raw/unpacked_raw.rs, fits/decode/mod.rs}`, `pipeline/mosaic_preparation.rs`.

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
- [ ] `image/pixel_flags.rs` `PixelFlags::dilate_window`, `image/cfa/cfa_image.rs:305` — each
  demosaic makes a new `DilationScratch`: up to `workers × 2 × height × 64` B of block ORs (8 MB
  at 24 MP on 16 workers), allocated for each frame and priced nowhere. Its callers
  (`io/raw/unpacked_raw.rs`, `io/fits/decode/mod.rs`, `pipeline/mosaic_preparation.rs`) reach
  the demosaic through `LoadCtx`. Target: the scratch on the owner that decodes frame after
  frame, through `LoadCtx`, and its `heap_bytes` in the decode's price.

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
- [ ] `math/wavelet/mod.rs:33` — `atrous_smooth` is a third separable-filter design beside the
  convolution kernels and the band filter, with `for_each_init(|| vec![0.0; width])` on each call
  (scales × channels for each op, and once for each MRS tile). Target: one row scratch from a
  pool its owner keeps, and the à trous pass on the convolution's row and column kernels with the
  dilation step, if a bench shows no loss.
- [ ] `drizzle/drizzle_shape.rs:104-105` — the input footprint is built by hand. Target:
  `FramePeek::footprint(workers)`, as every other entry uses.
