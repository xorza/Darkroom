# Review: lumos scratch and buffer pools

Whoever addresses an item deletes it.

Scope: `lumos/src/scratch_pool.rs`,
`lumos/src/star_detection/resources.rs`, `lumos/src/pipeline/detector_pool.rs`, and their users.
Paths below are relative to `lumos/src/`.

## Decided

- Leases are `Arc`-owned `Pooled<T>`, `'static`; no lifetime on the stage structs.
- Warp output planes come from a `ScratchPool<WarpBuffers>` on `FrameRegistrar`, not from slots.
- `reshape` drops the free list on any change of shape, for every `Scratch` type alike.

## Acquired contents are stale, and one path can return them

- [ ] `star_detection/background/background_estimate.rs:183-218` — `refine` acquires `sources`
  and writes it only inside the loop. At `iterations == 0` it returns the mask a previous frame
  left in the pool, and `PreparedFrame` uses it as the source mask. Today only
  `BackgroundConfig::validate` (`1..=10`, in another module) stops that. Target: `iterations:
  NonZeroUsize` in `BackgroundRefinement::Iterative` and `Refinement`, so the case does not exist.
