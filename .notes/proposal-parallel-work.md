# Proposal: parallel work through `common::par`

Closes the four issues the 2026-10-24 investigation confirmed: nested parallel
work (the MRS tile scratch and the `bg` bin map among it), the API surface that
drops feature-gated methods, and the compressed-read memory figure. Facts and
measurements: `docs/research/2026-10-24-parallel-work.md`. When this lands, each
lasting decision moves into the doc comments of `common::par` and of the code it
governs, and this file is deleted.

## Decisions

- **A parallel loop's body never starts parallel work.** A worker that blocks in
  a nested rayon call runs other jobs on its stack, so per-thread scratch is not
  exclusive and every per-thread memory figure is wrong. Refused: rayon's own
  answer, a value per split (`map_init`), because its count is unbounded by the
  thread count and it allocates per call; a second pool for inner work, because a
  waiting worker still runs its own pool's jobs; isolation, because rayon has none
  (oneTBB's `this_task_arena::isolate` is the reference fix). Research: "Rayon
  nests jobs on one stack".
- **All parallel work goes through `common::par`.** The root `clippy.toml` bans
  rayon's entry points (`par_iter`, `par_iter_mut`, `into_par_iter`,
  `par_chunks*`, `par_drain`, `par_bridge`, `par_sort*`, `join`, `scope`,
  `spawn`, `broadcast`, `ThreadPool::install`, `current_num_threads`,
  `current_thread_index`) and the per-split adapters (`map_init`, `map_with`,
  `for_each_init`, `for_each_with`, `try_for_each_init`, `try_for_each_with`);
  `common/clippy.toml` has none of them, as for I/O and threads today. `par_extend`
  is banned too, as it takes any `IntoParallelIterator`, a plain `Vec` among them;
  adapters that start nothing (`map`, `zip`, `enumerate`, `collect`) stay rayon's
  own. The bans land with the lumos move (batch 4), which they would fail before.
- **A run-time check at each entry, not a token.** Each entry point is a free
  function of `common::par` returning rayon's own iterator, and panics, in
  release too, when the caller is a pool worker: top-level code never runs on
  one, so a caller there is a loop's body. One read of rayon's thread-local a
  loop. Refused: a `Par` token every function on a path to a loop takes, which
  refuses nesting at compile time but threads a parameter through every
  signature and the public API (the user refused it); a flag a wrapper sets in
  each body, which `zip` and `chain` bypass as they consume a producer through
  `into_iter`. Research: Polars' pool check.
- **Rayon's global pool is the one pool.** An entry point returns an iterator
  its caller drives, and rayon drives it on the global pool off a worker, so a
  static API has no other pool to use. A one-item loop runs its body on its
  caller's thread, where a nested loop waits without stealing. `WorkerPool` is
  test-only: it runs a body as top-level code on a pool of a set size, marking
  that worker in a test-only `thread_local!`.
- **Frame slots run on plain threads.** `par::slots` — today's
  `concurrency::try_par_map_bounded` — runs each slot on a scoped
  `common::thread`, so a job's loops start off the pool and nothing nests on a
  worker's stack, frames included (rayon #1054 goes with it). Its cost: a slot's
  serial steps run beside the workers rather than as one of them.
- **Scratch is per worker, owned by the caller.** `PerWorker<S>` holds one slot
  for each pool thread and one for bodies run outside the pool, kept by the owner
  from call to call; a worker's body takes its slot through a checked borrow that
  panics on a second borrow. The no-nesting rule makes each slot single-user, so
  a working set is exactly `(par::threads() + 1) × scratch` and a call allocates
  nothing. Refused: an
  `UnsafeCell` slot, unsound the day a submodule's hidden parallel call nests;
  a scratch pool that grows on demand, whose peak is not a function of the thread
  count.
- **Thread counts come from `par`.** `par::threads()` replaces
  `rayon::current_num_threads()`, so a memory figure and the pool that spends it
  cannot disagree.
- **A submodule's parallel functions are entry points too.** `imaginarium`'s
  `Buffer2::interleave`, `deinterleave`, `Image::convert`, `convert_to` and
  `fits-well`'s compressed reads and writes join the `disallowed-methods` list,
  and the workspace calls them through `par` wrappers that run the same check.
  Refused: giving the submodules `common::par`, which would tie standalone crates
  to `common`. Their own serial mode stays an open request in each one's issue log.
- **`cargo drive surface` prints every build's lines.** An item's block is the
  union of its builds' lines, each line that not every build has tagged with the
  features that bring it, as items are tagged today. Refused: printing one block
  per build, which repeats every shared line. The fix lives in xorza/drive.
- **fits-well reports what a section read holds.** `ImageReader` gains the peak
  bytes of reading a region: the touched tiles' table rows and heap cells, the
  decode wave and its per-thread tile scratch. lumos's plan uses it in place of
  `2 × padded_data_bytes`. To be exact, the reader reads the tile descriptors
  once, when a compressed image is opened, and keeps them for the decode that
  reads them anyway. Refused: lumos deriving fits-well's internals, which drift;
  a header-only bound from `PCOUNT`, correct but loose by the tile count.
- **A compressed image's chunks align to its tile rows.** lumos rounds
  `rows_per_chunk` to whole tile rows, at least one, so each tile decodes once;
  today a chunk that cuts a tile row decodes those tiles again for the next chunk.
- **fits-well's decode keeps its buffers.** The wave buffer, its lengths and the
  per-thread tile scratch live on the reader's scratch and are reused by every
  section, as `section_shape` and `tile_rows` already are.

## Plan

Each batch ends green on `cargo drive check` and `cargo drive test` in the
workspace it touches (`cargo drive gate` for a submodule), and each item states
its proof.

### 1. `cargo drive surface` (xorza/drive)

- [x] Render an item as the union of its builds' lines, tagging each line that
  not every build has with its builds. Test: a fixture crate whose struct has a
  method under a feature renders that method, tagged, and the shared lines once.
- [x] Regenerate `fits-well/.notes/API_SURFACE.md` and
  `palantir/.notes/API_SURFACE.md`; check `write_compressed_image`,
  `write_compressed_table` and `system_clipboard` appear. Delete the issue.

### 2. `common::par`

- [x] Free functions `iter`, `iter_mut`, `into_iter`, `chunks`, `chunks_exact`,
  `chunks_mut`, `chunks_exact_mut`, `drain`, `extend`, `slots`, `threads`;
  `PerWorker<S>`; a test-only `WorkerPool`, which replaces `thread::pool`.
- [x] Tests: a loop started in a body panics, a zipped body's too, and `slots`
  started in one; `run` panics on a worker; `PerWorker` makes one slot per worker
  and one outside, kept across runs, each body taking its own, a one-item loop's
  body the outside one, and panics on a second borrow and past its pool; `slots`
  keeps the ordering, cap, carried-slot and lowest-failure tests, and a job runs
  loops of its own.

### 3. The two nestings

- [x] `wavelet::atrous_smooth_serial` beside the parallel form, one row kernel
  for both; the tile body smooths serially through a row its `TileScratch`
  keeps, so `heap_bytes` counts a row per scratch, not per thread. Test: the
  serial and parallel forms agree bit for bit.
- [x] `BinnedSamples::measure_in_turn` measures the bins one after another for
  MRS, whose σ is parallel; `measure` keeps the bins parallel for the mosaic's
  serial MAD. MRS's split is 4–9 % slower than the nesting it replaces,
  and the bins in parallel with a serial σ 2.2–2.4× slower than that
  (research: "Taking the nesting out of MRS"). The MRS issue is deleted.

The tile scratch moves into a `PerWorker`, and `heap_bytes` is held to the
counting allocator, in batch 4.

### 4. lumos onto `common::par`

Module group by module group, each a commit that builds:

- [ ] `concurrency` (`zeroed_in_parallel`, `slots`) and `math`.
- [ ] `image` (flags, flat gain, CFA, demosaic, linear image).
- [ ] `io` (FITS decode and write, RAW, flags and gain extensions) and `ingest`.
- [ ] `calibration`, `noise`, `frame`, `bg_mesh`.
- [ ] `star_detection`, `registration`, `drizzle`.
- [ ] `combine`, `pipeline`, `image_ops`.
- [ ] Every `map_init`/`for_each_init`/`try_for_each_init` becomes a
  `PerWorker` its owner keeps; every `current_num_threads()` becomes
  `par::threads()`. Tests: the memory-budget suite, and each changed
  `heap_bytes` against the counting allocator.
- [ ] The `clippy.toml` bans, each with its reason; `concurrency`'s bounded maps
  become `par::slots`.

### 5. Submodule calls, lens and darkroom

- [ ] The `imaginarium` and `fits-well` parallel functions in
  `disallowed-methods`, called through `par` wrappers that run the check.
- [ ] Rerun the nesting probe (method in the research note) over the lumos,
  lens and darkroom tests; it records nothing.

### 6. The compressed read (fits-well, then lumos)

- [ ] fits-well: the decode wave, its lengths and the tile scratch on the
  reader's scratch. Test: a second section read of the same image allocates
  nothing.
- [ ] fits-well: descriptors read once at open; `ImageReader` reports a region
  read's peak bytes. Test: for row-tiled, square-tiled and single-tile images,
  the figure equals the counting allocator's peak for that read.
- [ ] lumos: `rows_per_chunk` aligned to tile rows; the plan's compressed term
  from fits-well's figure. Test: a single-tile image decodes each tile once
  (decode count through the reader), and the plan's peak equals the measured
  peak. Delete the compressed-read issue.
