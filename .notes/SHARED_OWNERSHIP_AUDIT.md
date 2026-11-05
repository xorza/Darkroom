# Shared ownership audit

This audit examines each use of `Rc`, `Arc`, `Weak`, `RefCell`, `Cell`, `Mutex`, `RwLock`,
`OnceLock`, `OnceCell`, `LazyLock`, the atomics, `thread_local!`, the `tokio::sync` primitives
and the channels in the workspace. It also includes the submodules `palantir` and `fits-well`.
For each use, it answers three questions:

1. Is shared ownership necessary? Can one owner with `&` or `&mut`, a move, a scoped thread, a
   rayon reduce, a channel message or an index into an owning store replace it?
2. If sharing is necessary, is the primitive the standard one?
3. Is the use robust? The audit looks for races, lost wakes, deadlocks, `RefCell` panics,
   `Rc` cycles and missing bounds.

This is a temporary proposal. When a change lands, its lasting decision moves into the
`AGENTS.md` or the doc comment of the system that it governs. Then delete the change from this
file.

## Summary

- **No open bug.** The items below are simplifications: 3 uses that can go, and 1 that can
  change to a simpler or more standard primitive.

## Recommended order

1. Replace the atomic union-find in `lumos`.
2. Do the other simplifications in the order of this document.

## Shared ownership to remove

### lumos: atomic union-find in star labeling

- **Where:** `lumos/src/star_detection/labeling/union_find.rs` (the whole file),
  `labeler.rs:104-122` and `run.rs:157` (`RunJoin.union_find: &UnionFind`).
- **Why it is safe:** the parallel pass unions labels only inside one strip.
  `Strip::label_rows` calls `merge_with_prev` on its own rows. The only union across strips is
  `stitch`, which runs on one thread after the join. Thus, the CAS loops never contend.
- **Cost today:** one shared `next_label.fetch_add` per run, on one cache line that each worker
  touches, and false sharing on interleaved parent slots.
- **Replacement:** count the runs of each strip (`count_runs` already counts each row). Do a
  prefix sum of those counts to get the label bases. Give each run its base plus its index.
  Split the parent `Vec<u32>` into `&mut` slices, one for each strip, with a plain union-find in
  each. `stitch` and `build_label_map` then run on `&mut [u32]`.
- **Gain:** no atomics or orderings to examine, and provisional labels in raster order on any
  schedule.
- **Effort:** medium, about three files.

### lumos: `AtomicFlagCounts` and `fallback_pixels`

- **Where:** `lumos/src/image/flags/flag_counts.rs:94-107`. Its only user is
  `combine/frame_cache/mod.rs:350-352,573-575`, which also holds the `AtomicU64`
  `fallback_pixels`.
- **Replacement:** change the row loop's `for_each` into
  `.map(|row| tallies).reduce(Default::default, merged)` with the existing
  `FlagCounts::merged`. Fold the result of each chunk into locals of the `FnMut` chunk callback.
- **Gain:** one type less, and 17 fewer atomic read-modify-writes for each row. The sums are
  already deterministic, so the gain is less shared state.
- **Effort:** small.

### lumos: `deblended_components`

- **Where:** `lumos/src/star_detection/star_detector/stages/detect/mod.rs:145` (`AtomicUsize`).
- **Replacement:** `par::chunks(..).enumerate().map(|..| count).sum()`.
- **Effort:** trivial.

## Primitives to simplify or make standard

### lumos: `ScratchPool` used as scratch for each worker

- **Sites:**
  - `combine/frame_cache/mod.rs:442` (a pool made for one call)
  - `bg_mesh/mod.rs:123,139`
  - `registration/resample/mod.rs:221,230`
  - `star_detection/detection_filter/mod.rs:127`
  - `image/flags/pixel_flags.rs:261,266`
  - `drizzle/accumulator/output_band.rs:134,352`
- **From → to:** an `Arc<Mutex<Vec>>` lease for each item → `common::PerWorker::get()`.
- **Why:** each item takes the shared lock two times and does an `Arc` increment and decrement on
  the same two cache lines. `PerWorker` gives each worker its own slot, aligned to 128 bytes.
- **Two ways to do one thing:** `scratch_pool/mod.rs:9-12` says that `PerWorker` is for scratch
  that lives for one call. But `deblend/deblenders.rs:18` and `calibration/cosmic_ray/mono.rs:78`
  keep a `PerWorker` on an owner across runs. Thus, the split that the doc states does not exist.
- **Keep `ScratchPool`** for planes held across stages: the `DetectionResources` floats, bitmasks
  and surfaces, and the `Pooled` fields in `prepared_frame`, `bg_estimate` and `sky_noise`.
- **Caveats:** threads outside the pool share one `PerWorker` slot (the blocking lock in
  `per_worker.rs:62`), so the owner must belong to one slot thread. The memory accounting changes
  from `workers` to `workers + 1` (`heap_bytes` in `resources.rs`).

## Sources

- Mara Bos, *Rust Atomics and Locks*, chapters 2 and 3: stop flags, release and acquire,
  happens-before.
- rayon docs: `reduce`.
