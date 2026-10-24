# Parallel work, the API surface and the compressed read (2026-10-24)

Facts gathered for `.notes/proposal-parallel-work.md`. Workspace at `584c868d3`,
rayon 1.12.0, rayon-core 1.13.0, cargo-drive at xorza/drive `58666cd`.

## Rayon nests jobs on one stack

- A worker that blocks in a rayon call (a `join` under a parallel iterator, a
  `scope`) runs other queued jobs on the same stack until it is released. Code
  that is not recursive still ends up nested this way. The maintainers give this
  as the reason `map_init` makes one value per split, not per thread, and why a
  thread-local scratch is not exclusive while nested work can run.
  [forum: Nested parallelism and Rayon](https://users.rust-lang.org/t/nested-parallelism-and-rayon/84210),
  [rayon #941](https://github.com/rayon-rs/rayon/issues/941),
  [rayon FAQ](https://github.com/nikomatsakis/rayon/blob/master/FAQ.md).
- Rayon has no work isolation. A second pool does not isolate: a worker that waits
  on a cross-pool `install` keeps running jobs of its own pool
  (`rayon-core/src/registry.rs`, `in_worker_cross` → `wait_until`).
- oneTBB solves the same problem with `this_task_arena::isolate`: a thread inside
  the region runs only tasks of that region, so outer tasks cannot land on its
  stack and thread-local state stays its own. Before it, a separate arena was the
  workaround, at a noted overhead.
  [oneTBB work isolation](https://www.intel.com/content/www/us/en/docs/onetbb/developer-guide-api-reference/2023-0/work-isolation.html).
- Spawned tasks that each run a parallel iterator can all wait until every inner
  loop is done. [rayon #1054](https://github.com/rayon-rs/rayon/issues/1054).
- The newer data-parallel crates (Paralight, parallel_tasker) and crossbeam offer
  no isolation either. [wild-linker discussion #1072](https://github.com/wild-linker/wild/discussions/1072).

## Where lumos nests parallel work

Method: rayon and rayon-core patched through `--config patch.crates-io`, with a
separate target directory, so no file of the workspace changed. Every leaf fold
of `bridge_producer_consumer` and `bridge_unindexed` pushes its consumer's
`type_name` on a thread-local stack, and each spawned scope body pushes its own;
every job a worker takes from a queue (`JobRef::execute`) pushes a barrier. A
bridge that starts while the top of the stack is a leaf, not a barrier, is a
nesting; each (outer, inner) pair of type names is recorded once. Run over the
`lumos` and `lens` test targets with `--features lumos/ml`: 1512 + 61 + 7 + 15
tests passed; `memory_budget` failed on the probe's own allocations, and passes
without it. Without the barrier, a worker waiting in a join that took a queued
job of another concurrently running test showed as a false nesting.

| Outer body | Parallel work it starts |
| --- | --- |
| `NoisePlane::coefficients`, the `map_init` tile body (every caller: frame stats, denoise, star-detection channel noise) | `wavelet::atrous_smooth` |
| `NoisePlane::bg`, the map over gain bins (`BinnedSamples::measure`) | `mrs_noise::sigma`, `mrs_noise::support_sigma` |
| `concurrency::try_par_map_bounded` scope bodies: frame loading (`load_tiered`), detection, registration and warp, one-pass stacking | the whole per-frame pipeline: decode, calibration, cosmic rays, demosaic, frame stats, noise, background mesh, star detection, warp |
| tests only: `stage_counter::a_callback_that_uses_rayon_completes`, `star_detection::pool_working_set` | their own parallel calls |

No nesting was recorded inside `fits-well` or `imaginarium`, nor a lumos body
that starts one of their parallel functions. The `real-data` tests did not run.

- Production rayon entry points in lumos, by method: `par_chunks_mut` 56,
  `par_iter_mut` 17, `par_chunks` 16, `par_iter` 15, `into_par_iter` 13,
  `par_extend` 4, `par_drain` 2, `par_chunks_exact_mut` 1, `rayon::scope` 1;
  58 files. `rayon::current_num_threads()` is read in 9 places.
- Parallel functions of the submodules lumos calls: `imaginarium`
  `Buffer2::interleave`, `Buffer2::deinterleave`, `Image::convert`,
  `Image::convert_to`; `fits-well` with `parallel` (a default feature), the
  compressed image and table decode behind `ImageReader::view`, `read`,
  `physical_into`, `FitsReader::read_table*`, and the compressed writes.
- lens runs each lumos call inside `common::task::spawn_blocking`, so lumos's
  parallel loops start on a blocking thread and run on rayon's global pool.

### Rerun after the move to `common::par`

The same probe at `2cdbead6b` with the fits-well wrappers uncommitted, over the
`lumos` (`ml`, `internals`), `lens` and `darkroom` (`internals`) test targets:
1804 tests passed, `memory_budget` among them. One nesting was recorded,
`stage_counter::a_callback_that_uses_rayon_completes`, the test that calls
rayon directly to prove a callback may. The production nestings of the table
above are gone; the tests that prove a nested start panics stop at the check,
before their inner loop starts.

## MRS tile scratch

`NoisePlane::coefficients` makes a `TileScratch` in `map_init`, one for each
split. A worker inside a tile body waits in `atrous_smooth`'s join and can take
another split of the same tile list, which makes another scratch. The bound is
one scratch for each tile (at most 16); `mrs_noise::heap_bytes` counts
`threads.min(tiles)`.

## cargo drive surface

`src/surface/mod.rs` renders each path's block from the first build that
exports it (`blocks` collected in reverse build order into a `BTreeMap`), the
build with no features where it has one. `tag` adds "[shape differs by
feature]" when the builds' blocks differ, but no build's extra lines are
printed. Missing as a result: fits-well's `FitsWriter::write_compressed_image`
and `write_compressed_table`, palantir's `OffscreenHostBuilder::system_clipboard`.
fits-well's `FitsReader` carries no tag at `995766f`.

## What a compressed section read holds

lumos reads an image a chunk of rows at a time (`plan.rows_per_chunk`, from
`CHUNK_BYTES`) through `ImageReader::view`. For a tiled-compressed image,
fits-well at `995766f`:

- `FitsReader::image` reads the header only; no tile descriptor is read until a
  section is.
- A section reads the table rows of the tiles it touches and their heap cells
  (`image_reader.rs`, `tiles` → `table_rows(TableRows::Picked)`).
- The decode allocates, on every call, a wave buffer of `wave_len ×
  max_tile_elements` samples, with `wave_len = max(1, 4 MiB / tile slot bytes)`,
  and a `lengths` vector; each rayon split of the wave makes a
  `TileScratchSet` in `try_for_each_init` (`image_decode_plan.rs`,
  `decode_waves`).
- A tile is the compression unit, so every tile a section touches is decoded
  whole: a chunk that cuts a tile row decodes those tiles again for the next
  chunk.

lumos's plan adds `2 × padded_data_bytes` for a `CompressedImage`
(`decode/plan.rs`), a figure none of the above derives.

## Taking the nesting out of MRS

`cargo drive bench mrs_estimate`, 16 threads, the medians of criterion's runs:
the tree before, then with the tile body's smoothing serial and the gain bins
measured one after another (each bin's σ parallel), then, for the bins alone, the
other way round (bins in parallel, each σ serial).

| Case | Nested | Serial tile, serial bins | Parallel bins, serial σ |
| --- | ---: | ---: | ---: |
| `mrs_estimate/1024` | 14.83 ms | 14.80 ms | |
| `mrs_estimate/4096` | 18.45 ms | 18.70 ms | |
| `mrs_estimate_split/1024` | 24.96 ms | 27.30 ms | 60.42 ms |
| `mrs_estimate_split/4096` | 23.74 ms | 24.83 ms | 60.04 ms |

Eight bins hold a 1024² frame's coefficients; serial within a bin, the support
iteration's passes leave half of 16 threads idle and lose the share-level
parallelism each pass had.
