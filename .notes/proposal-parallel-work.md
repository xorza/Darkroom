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
- **A token proves the caller is not a loop body.** Every entry point is a method
  of `Par<'p>`, which is `!Send + !Sync`; rayon requires each body to be
  `Send + Sync`, so a body cannot hold the token and a nested loop does not
  compile. A function that runs parallel work takes `par: &Par<'_>`. Refused: a
  run-time flag in the wrappers, which needs a `thread_local!` and catches only
  what tests run; tokens kept in existing context structs, which would make those
  structs `!Sync` for every body that reads their other fields.
- **A static assertion proves the token stays out.** `assert_not_impl_any!(Par:
  Send, Sync)` in `common`'s tests, which with rayon's bounds is the whole
  property. Refused: `compile_fail` doc tests, which need rustdoc and the doc tests
  in every gate; `trybuild`, a new dependency whose saved compiler output breaks
  with each rustc.
- **Only a pool hands out a token.** `WorkerPool::new` replaces
  `common::thread::pool`; the pool is `Clone + Send + Sync`, and
  `WorkerPool::run(|par| …)` installs the work and passes the first `Par`. `run`
  asserts, in release, that it is not on a rayon worker: a body that captured a
  pool is the one path the types leave open, and the check costs one read per run.
  `Par` has no public constructor; tests and benches make a `WorkerPool` of their
  own. Named for its workers, as darkroom already has a `Pool` of its own.
- **darkroom owns the one compute pool.** `RuntimeLibrary` makes it and hands a
  clone to `astro_library` and `image_library`; a lens node calls `pool.run`
  inside its `spawn_blocking`. The global rayon pool is no longer used. Refused:
  a pool per lumos call, which would oversubscribe the cores when nodes run side
  by side.
- **Frame slots are the one nesting allowed.** `Par::slots(len, &mut slots, job)`
  — today's `concurrency::try_par_map_bounded` — gives each job its own `&Par`:
  each slot owns its frame's memory, so a slot job on another's stack stays within
  the slot count. Its cost, a frame finishing only when the jobs above it on the
  stack do, is accepted and documented. Research: rayon #1054.
- **Scratch is per worker, owned by the caller.** `PerWorker<S>` holds one slot
  for each pool thread, kept by the owner from call to call; a body takes its
  thread's slot through a checked borrow that panics on a second borrow. The
  no-nesting rule makes each slot single-user, so a working set is exactly
  `par.threads() × scratch` and a call allocates nothing. Refused: an
  `UnsafeCell` slot, unsound the day a submodule's hidden parallel call nests;
  a scratch pool that grows on demand, whose peak is not a function of the thread
  count.
- **Thread counts come from the token.** `par.threads()` replaces
  `rayon::current_num_threads()`, so a memory figure and the pool that spends it
  cannot disagree.
- **A submodule's parallel functions are entry points too.** `imaginarium`'s
  `Buffer2::interleave`, `deinterleave`, `Image::convert`, `convert_to` and
  `fits-well`'s compressed reads and writes join the `disallowed-methods` list,
  and the workspace calls them through functions that take `&Par`. Refused:
  giving the submodules the token, which would tie standalone crates to
  `common`. Their own serial mode stays an open request in each one's issue log.
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

- [x] `WorkerPool`, `Par<'p>`, `PerWorker<S>` in `common/src/par/`, one file
  each; `WorkerPool::new` replaces `thread::pool`. `Par` methods: `iter`,
  `iter_mut`, `into_iter`, `chunks`, `chunks_exact`, `chunks_mut`,
  `chunks_exact_mut`, `drain`, `extend`, `slots`, `threads`.
- [x] Tests: `Par` is neither `Send` nor `Sync`; `run` panics on a worker;
  `PerWorker` makes one slot per worker, kept across runs, each body taking its
  own, and panics on a second borrow, off a worker and past its workers; `slots`
  keeps the ordering, cap, carried-slot and lowest-failure tests, and a job's own
  `Par` runs loops.

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
counting allocator, in batch 4, once `coefficients` has a `Par`.

### 4. lumos onto `Par`

Module group by module group, each a commit that builds:

- [ ] `concurrency` (`zeroed_in_parallel`, `slots`) and `math`.
- [ ] `image` (flags, flat gain, CFA, demosaic, linear image).
- [ ] `io` (FITS decode and write, RAW, flags and gain extensions) and `ingest`.
- [ ] `calibration`, `noise`, `frame`, `bg_mesh`.
- [ ] `star_detection`, `registration`, `drizzle`.
- [ ] `combine`, `pipeline`, `image_ops`.
- [ ] Every `map_init`/`for_each_init`/`try_for_each_init` becomes a
  `PerWorker` its owner keeps; every `current_num_threads()` becomes
  `par.threads()`; every `heap_bytes` takes its thread count from the same
  `Par` the run uses. Tests: the memory-budget suite, and each changed
  `heap_bytes` against the counting allocator.
- [ ] lumos's public functions that run parallel work take `&Par`.
- [ ] The `clippy.toml` bans, each with its reason; `concurrency`'s bounded maps
  become `Par::slots`.

### 5. Submodule calls, lens and darkroom

- [ ] The `imaginarium` and `fits-well` parallel functions in
  `disallowed-methods`, called through `&Par` functions.
- [ ] `RuntimeLibrary` owns the `Pool`; `astro_library` and `image_library` take
  it; `run_cancellable` and the other `spawn_blocking` sites run inside
  `pool.run`. Test: a node graph test runs a lumos node and an image node side by
  side on one pool.
- [ ] Rerun the nesting probe (method in the research note) over the lumos,
  lens and darkroom tests; it records only `Par::slots` bodies.

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
