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
- **The demosaic's dilation scratch kept on `LoadCtx`.** It puts a private field in a public
  struct that callers build with `..Default::default()`, to save a few megabytes of page faults
  beside a demosaic of the whole frame. The scratch is priced in `FrameInfo::demosaiced` instead.
- **The drizzler keeping the output sums its tier returns, for the next frame.** It holds
  C + 2W output planes through the next frame's decode and statistics, which no stage charges,
  to save their page faults; `tests/memory_budget` measured a spilled drizzle past its budget.
  DESIGN.md refuses the same trade for the cosmic-ray detector.
- **One flat buffer for an RCD tile's 33 planes.** A tile is made once for each worker of a
  frame, not for each tile, and its kernels borrow its planes apart through `get_disjoint_mut`; a
  flat buffer trades that for `split_at_mut` chains in a vector kernel, to save about 33
  allocations a worker beside a demosaic of the whole frame. The X-Trans tile is priced by its
  passes' directions instead of the most.
- **A row scratch for the à trous pass from a pool its owner keeps.** No type spans the calls;
  one row a worker for each call is what it allocates now.
- **Moving or borrowing the `ImageMetadata` the stored image, the kept frame's reuse, the loader
  and the drizzle clone.** Each has two owners or runs once a run; only the commit's copy is
  per frame, and it borrows now.
- **Pricing the one-pass warp scratch beside `prepared.kept` only.** The worker keeps its
  `FlagScratch` from one light to the next, so the scratch is also held during the next decode.
  The current sum is correct.
- **Reusing the reference's KdTree and triangles across registrations.** This saves CPU, not
  memory, and is out of scope.

- **An `Arc`'s identity or count as the owner of a flat gain.** A short-lived clone, or two
  equal grids decoded apart, fools either. The producer states it (batch 11).
- **A warped gain computed when the combine reads it.** No pipeline found does: LSST, JWST,
  SWarp and DrizzlePac store each frame's noise plane and warp it with the image
  (`lumos/research/2026-10-23-flat-gain-practice.md`).
- **Every frame owning its source gain, as JWST's `VAR_FLAT`.** A calibrated mosaic would pay a
  copy of the master's grid for each light.
- **A header read of every file before the plan, to count the distinct gains.** It reads each
  file twice. The plan charges one gain for each frame the cache does not hold instead.

## Plan

Each batch leaves `cargo drive check` and `cargo drive test` green. A batch that changes a
kernel or a hot loop runs its benches before and after (`cargo drive bench <group>`),
and a slowdown is a defect of the batch. A batch that changes a vector kernel runs the inline
check in `lumos/AGENTS.md`. When a batch lands, its lasting decision goes into
`lumos/DESIGN.md` as a bullet, and its items are deleted here.

Batches 1, prices that match the allocations, 2, streaming FITS output, 3, the detection working
set, 4, the warp working set, 5, each input read once, 6, one stored-frame writer, and 11, shared
source gains, landed, and 7, reuse across frames and jobs, 9 but one item, and 10,
consolidation. The other batches are independent.

### 8. Raster decode bound

A bug: an input reaches an allocation that aborts the process. Files:
`imaginarium/src/image/{mod.rs, io.rs}`, `io/raster.rs`. Facts:
`lumos/research/2026-10-23-raster-decode-bounds.md`.

Decided: an admission callback, committed on imaginarium `main`. Refused: a limits value (one
number cannot state lumos's peak, 2× to 5× the decode by sample type), a probe then a decode (two
opens, and a file replaced between them decodes unbounded), and a header read in lumos through
`tiff` (a second decoder, the same race, previews left unbounded).

- [ ] `imaginarium/src/image/io.rs:86-97` and `io/raster.rs:30-41,74-78,136` — a raster TIFF
  decodes with `Limits::unlimited()`, and the `MAX_SIDE` check runs after the decode. A crafted
  header makes the decoder try to allocate up to 65535² × channels × 4 bytes. Target:
  `Image::decode_admitted(reader, format, admit: impl FnOnce(&ImageDesc) -> Result<(), E>)`
  reads the header, calls `admit`, and decodes on the same decoder: TIFF through
  `Decoder::dimensions` before `read_image`, PNG and JPEG through `ImageReader::into_decoder`
  and `DynamicImage::from_decoder`, with the `image` crate's 512 MiB default lifted so the
  admission alone bounds them. Its error tells a refusal (`E`) from a decode error. lumos's
  linear and preview paths admit `MAX_SIDE` and `OverBudget::check` on their whole peak.

### 9. Cold image operations

Once for each master, but each holds hundreds of MB. Files: `io/raster.rs`,
`image/linear_pixels.rs`.

- [ ] `io/raster.rs:107` — `LinearImage::from(&decoded)` copies even an `L_F32` image. Target:
  imaginarium's `planar_conversions!` gains `TryFrom<Image> for [Buffer2<T>; N]` beside the
  borrowing one, N = 1 moving its buffer, in batch 8's commit; lumos takes `From<Image> for
  LinearImage`, and the linear path's admission charges 1× for mono f32, 2× otherwise.
