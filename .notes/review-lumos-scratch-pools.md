# Review: lumos scratch and buffer pools

Whoever addresses an item deletes it.

Scope: `lumos/src/scratch_pool.rs`, `lumos/src/concurrency/job_scratch_pool.rs`,
`lumos/src/star_detection/resources.rs`, `lumos/src/pipeline/detector_pool.rs`, and their users.
Paths below are relative to `lumos/src/`.

## Decided

- Leases are `Arc`-owned `Pooled<T>`, `'static`; no lifetime on the stage structs.
- Warp output planes come from a `ScratchPool<WarpBuffers>` on `FrameRegistrar`, not from slots.
- `reshape` drops the free list on any change of shape, for every `Scratch` type alike.

## Target shape

`ScratchPool<T: Scratch>` and its lease `Pooled<T>` live in `scratch_pool.rs`, and the detection
planes use them. The rest moves onto it, adding `ScratchPool::clear` and `Pooled::into_inner` with
their first callers:

| Today | Target |
|---|---|
| `JobScratchPool<CombineScratch>` + `resize(frame_count)` in the init | `ScratchPool::new(frame_count)`, `fit` resizes |
| `JobScratchPool<InterpolateScratch>` + `resize(tiles_x)` per row | `Shape = usize` (tiles_x), fitted once per lease |
| `JobScratchPool<CornerLattice>` + `reset(width)` | `Shape = usize` (width), `fit` resets |
| `JobScratchPool<DeblendBuffers / TileScratch / RadialScratch / RowScratch>` | `Shape = ()` |
| `Vec<Option<WarpBuffers>>` slots + `StoredWarp::reusable` | `ScratchPool<WarpBuffers>`, `Shape = ImageDimensions` |
| `Labeler::spare` + `LabelMap::release_to_pool` | `Labeler::label(&mut self, ..) -> &LabelMap` |

## Two pool types for one mechanism

- [ ] `concurrency/job_scratch_pool.rs` — a second free list beside `ScratchPool`, with its own
  contents rule and test internals. Target: its users move to `ScratchPool<T: Scratch>` and the
  file goes.
- [ ] `concurrency/job_scratch_pool.rs:31` — `T: Default` builds a value with no shape, so each
  caller fits it by hand after the acquire: `combine/frame_cache/mod.rs:428-433`
  (`resize(frame_count)` in the init), `star_detection/background/background_estimate.rs:266`
  (`resize(tiles_x)` on every row), `drizzle/accumulator/output_band.rs:133-134` (`reset(width)`).
  Target: `Scratch::fit` on acquire, the shape held by the pool.
- [ ] `pipeline/detector_pool.rs:8-15` — the doc warns that `JobScratchPool<StarDetector>`
  compiles and builds detectors from the default config. The trap comes from `T: Default`.
  Target: `StarDetector` does not implement `Scratch`, so the wrong pool does not compile and the
  warning goes.
- [ ] `star_detection/resources.rs:74`, `star_detection/background/workspace.rs:15` — no `clear`
  on `JobScratchPool`, so owners replace the pool with `default()`. Target: `ScratchPool::clear`.
- [ ] `concurrency/job_scratch_pool.rs:86` — `job_count` and `all_by` are free functions in
  `#[cfg(test)]` internals. Target: `ScratchPool::pooled`, which exists.

## Worker scratch travels with the warp's output planes

- [ ] `registration/resample/mod.rs:98-99`, `pipeline/frame_parking.rs:104-131` — `WarpBuffers`
  carries `rows: JobScratchPool<RowScratch>`, the scratch of the workers that fill it. The
  resident tier turns the planes into the stored frame and drops `rows` with them, so each
  resident frame's warp builds its row scratch again on every rayon split. Target: one
  `ScratchPool<RowScratch>` on `FrameRegistrar`, shared by every worker; `WarpBuffers` holds
  planes and flags only.
- [ ] `pipeline/align.rs:121`, `pipeline/light_source.rs:602`, `pipeline/frame_parking.rs:48-51,123,136`,
  `pipeline/frame_registrar.rs:83,138-140`, `pipeline/detector_pool.rs:50-57` — a hand-written
  free list: `Vec<Option<WarpBuffers>>` per slot, `take()`/`unwrap_or_else(new)`,
  `StoredWarp::reusable` to give the set back, and `DetectorPool::paired_with` to zip the two
  slot kinds. Target: `ScratchPool<WarpBuffers>` on `FrameRegistrar`, `Shape = ImageDimensions`.
  The spill tier drops the lease, the resident tier calls `into_inner`. `park` takes no buffer
  argument, `paired_with` and `reusable` go, and the bounded maps take the detectors alone as
  slots. The pool holds at most the warp width of sets, as the slots do, and is dropped at the
  same point before the combine.

## The labeler lends its output by value

- [ ] `star_detection/labeling/mod.rs:46-58`, `star_detection/labeling/labeler.rs:37,200` —
  `LabelMap::from_pool` returns an owned map built from `Labeler::spare`, and the caller must hand
  it back through `release_to_pool` or the next frame allocates it again. Target:
  `Labeler::label(&mut self, mask, connectivity) -> &LabelMap`. The callers
  (`star_detector/stages/detect/mod.rs:70-75`, `tests/real_data.rs`) read only disjoint fields
  (`resources.deblend`) while they hold the map. `from_pool`, `release_to_pool`, `spare` and
  `recycle` go.

## Acquired contents are stale, and one path can return them

- [ ] `star_detection/background/background_estimate.rs:181-216` — `refine` acquires `sources`
  and writes it only inside the loop. At `iterations == 0` it returns the mask a previous frame
  left in the pool, and `PreparedFrame` uses it as the source mask. Today only
  `BackgroundConfig::validate` (`1..=10`, in another module) stops that. Target: `iterations:
  NonZeroUsize` in `BackgroundRefinement::Iterative` and `Refinement`, so the case does not exist.

## Per-call scratch in a loop the frame runs many times

- [ ] `calibration/cosmic_ray/mono.rs:289,454` — `median_window_rows` builds a `WindowScratch`,
  and `replace_flagged` a `Vec::with_capacity(25)`, in each rayon split of each call. The calls run
  four per pass, per iteration, per Bayer phase, on a detector whose doc promises a reused working
  set. Target: a `ScratchPool<WindowScratch>` on `MonoDetector`. The other `for_each_init` sites
  (normalization, defect map, dark scale, pixel flags, black level, wavelet) run once per frame and
  stay, as the pool's doc already rules.
