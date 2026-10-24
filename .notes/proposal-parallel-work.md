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
- **Scratch is per worker or leased per item, never per split.** A `fold`'s
  accumulators live until its `reduce`, so a value per split counts the splits,
  which follow the schedule: the deblender's leased sets grew a detector's heap
  frame by frame. The init adapters (`map_init`, `for_each_init` and kin) and the
  folds are banned with the entry points. A body leases from a pool (`acquire`,
  a surface `reader`), which allocates nothing warm and holds one lease a running
  body; or takes its worker's slot of a `PerWorker`, kept on an owner across calls
  where one exists (the deblenders, the cosmic-ray windows) and made for the call
  otherwise. A slot whose value the figures charge per worker (the MRS tiles, the
  demosaic tiles) starts empty and is made on first use: a split loop runs on the
  workers alone and an unsplit one on its caller alone, so no more are made than
  the workers. Refused: an `UnsafeCell` slot, unsound the day a submodule's hidden
  parallel call nests.
- **Thread counts come from `par`.** `par::threads()` replaces
  `rayon::current_num_threads()`, so a memory figure and the pool that spends it
  cannot disagree.
- **A submodule's parallel functions are entry points too.** fits-well's
  methods that decode or encode tiles in parallel are in `disallowed-methods`,
  and lumos calls them through `io::fits::tile_parallel`, which runs
  `par::enter` first, for a plain image too, so a test on a plain file refuses
  what a compressed one would. Each is an inherent method, so the ban reaches
  every call. Its cost: the list is kept by hand, so a parallel method fits-well
  adds later needs its entry, and the nesting probe finds a miss. Refused: giving fits-well `common::par`, which would tie a
  standalone library to `common`; a check inside fits-well, which cannot see the
  test-only top-level mark, so the memory harness would panic.
- **imaginarium moves into the workspace.** Three of its parallel paths are
  `From`/`TryFrom` impls, which no ban can name: clippy resolves them to the
  trait's method. As a member it calls `common::par` itself, so the check covers
  every path, and a parallel function it adds later. No other crate depends on
  it. Refused: bans and wrappers as for fits-well, which leave those three open
  and need a list kept by hand.
- **`cargo drive surface` prints every build's lines.** An item's block is the
  union of its builds' lines, each line that not every build has tagged with the
  features that bring it, as items are tagged today. Refused: printing one block
  per build, which repeats every shared line. The fix lives in xorza/drive.
- **fits-well bounds what a run of views holds.** `ImageReader::view_bytes` gives,
  by owner, what the reader keeps and what the handle holds while it reads: the tile
  index, the touched tiles' rows and heap cells, the decode's wave, and a tile set for
  each worker that may take a run of the tiles, each set reserved to its codec's
  buffers before its first tile. lumos's plan uses it in place of
  `2 × padded_data_bytes`. The tile index is read once, when a compressed image opens,
  and kept for the decode that reads it anyway. Refused: lumos deriving fits-well's
  internals, which drift; a header-only bound from `PCOUNT`, correct but loose by the
  tile count; an exact figure, which fixed the split of each wave across the workers
  and gave up rayon's balancing, and buffers kept from one read to the next, which
  needed that split — the bound serves a budget as well, at less machinery.
- **A compressed image's chunks align to its tile rows.** lumos rounds
  `rows_per_chunk` to whole tile rows, at least one, so each tile decodes once;
  before, a chunk that cut a tile row decoded those tiles again for the next chunk.
- **A compressed image is read once.** A checksummed read sums the tile index from
  memory and each section's heap cells as it fetches them, so the finish reads only
  the block fill; before, it read the whole data unit again.

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

- [x] Every rayon entry point in lumos through `par`; `concurrency` dissolved —
  `slots_owned` into `common::par`, the rest beside its users, its huge-page
  prefault kept, in safe code, as a 50 % decode slowdown without it showed.
- [x] The deblender's per-split `fold` becomes `Deblenders`: a set a worker and its
  regions gathered back into the components' order; the memory figures count the
  set outside the pool.
- [x] Every init adapter becomes a lease per item or a `PerWorker`; every
  `current_num_threads()` becomes `par::threads()`.
- [x] The `clippy.toml` bans: the entry points, the init adapters and the folds.
- [x] The memory harness runs the test's body as marked top-level code on worker
  0, so its jobs take that worker's queue, not rayon's injector, whose blocks
  landed in whichever measurement filled one.

### 5. Submodule calls, lens and darkroom

- [x] fits-well's parallel methods in `disallowed-methods`, called through
  `tile_parallel`. Test: a loop's body that reads a plain image through it
  panics, and the same read at top level returns the samples.
- [ ] imaginarium a workspace member: its own `common` module renamed, its
  file reads and writes through `common::fs`, its rayon calls through
  `common::par`, its names and lints held to the workspace's.
- [x] Rerun the nesting probe (method in the research note) over the lumos,
  lens and darkroom tests: it records the one test that nests on purpose
  (research: "Rerun after the move to `common::par`"). Again after imaginarium
  moves.

### 6. The compressed read (fits-well, then lumos)

- [x] fits-well: the tile index read once at open; sections fetch only heap cells;
  tile sets reserved to the per-codec buffers; gzip through one reused inflate
  state. Tests: gzip framing and trailer cases; the fetched-byte counts.
- [x] fits-well: `view_bytes`, a bound by owner. Tests: on four workers the peak is at
  most the figure; on one, over a run that grows each buffer once, the figure is the
  peak, for row, square and single tiles through every codec, from memory and from a
  stream.
- [x] fits-well: a checksummed compressed read reads its unit once. Test: sections in
  tile order fetch the padded unit once; a whole read, the same.
- [x] lumos: `rows_per_chunk` aligned to tile rows; the plan's compressed term from
  fits-well's figure and its finish bound. Test: row tiles, tiles 100 rows high and
  one tile give chunks of 256, 200 and 300 rows, and the peak is the decoded planes, a chunk's words, the
  figure and the finish. Bench (research `2026-10-24-read-bytes.md` in fits-well):
  a stream's RICE section reads faster than before; a resident source's sections
  are summed at the finish, as summing between decodes cost 16 %.
