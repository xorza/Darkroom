# Design review: prism/src/gui/graph_background.rs  (2026-04-22)

## Current design

`GraphBackgroundRenderer` draws a tiled dot pattern behind the graph. It owns two lazy-init caches: a single-tile `TextureHandle` (the dot image, built once) and an `Arc<Mesh>` (a 4-vertex quad that's mutated in place every frame to reposition UVs for the current pan/zoom). `render()` ensures both exist, then `draw_tiled()` computes the per-frame UV transform, overwrites all four vertices via `Arc::make_mut`, and submits the mesh via `Shape::mesh(Arc::clone(...))`. The pure helper `wrap_scale_multiplier` — well-tested — picks a power-of-2 density multiplier that keeps the effective on-screen dot spacing within `[0.5, 3.0]` as the user zooms.

The module is self-contained: one public struct, one public method, one caller. Style pulled from `gui.style.graph_background`. No cross-frame state beyond the two caches. The manual `Debug` impl skips the fields because `TextureHandle` / `Mesh` don't derive `Debug`.

## Overall take

Mostly right — self-contained, one clear job, the non-obvious density-wrap math is extracted as a pure function and tested. Only real design question is whether the `quad_mesh` cache earns its keep. One minor naming cleanup.

## Findings

### [F1] `quad_mesh` cache is over-engineered
- **Category**: State
- **Impact**: 2/5 — removes cross-frame state and the `Arc::make_mut` COW dance; code gets shorter and more obviously correct
- **Effort**: 1/5 — in-file refactor, no API change
- **Current**: Line 14 stores `Option<Arc<Mesh>>`. First `render()` creates the mesh + indices (lines 33–38). Every subsequent `render()` does `Arc::make_mut(self.quad_mesh.as_mut().unwrap())` (line 114), overwrites all four vertices (lines 115–131), then submits via `Shape::mesh(Arc::clone(self.quad_mesh.as_ref().unwrap()))` (line 135).
- **Problem**: A 4-vertex + 6-index quad is ~80 bytes. Caching it across frames buys nothing — `Arc::make_mut` frequently has to clone anyway because the submitted `Arc` from the previous frame is still referenced by egui's paint queue until the frame flushes. The cache adds: an Option field with its unwrap discipline, a manual `Debug` impl (Mesh doesn't derive Debug), and the slight cognitive load of "why is this Arc mutable?". Net of the cache: a solution for a problem that doesn't exist.
- **Alternative**: Construct the mesh inside `draw_tiled` each frame and submit directly:
  ```rust
  let mut mesh = Mesh::with_texture(texture.id());
  mesh.vertices = vec![ /* 4 Vertex with pos+uv+color */ ];
  mesh.indices = vec![0, 1, 2, 0, 2, 3];
  gui.painter().add(Shape::mesh(Arc::new(mesh)));
  ```
  Removes the `quad_mesh: Option<Arc<Mesh>>` field, removes the first-frame init block, removes the manual `Debug` impl (struct can now `#[derive(Debug)]` since only `Option<TextureHandle>` remains and that derives Debug).
- **Recommendation**: Do it.

### [F2] Density-wrap range is a magic pair of locals
- **Category**: Types / naming
- **Impact**: 1/5 — readability
- **Effort**: 1/5 — extract two consts
- **Current**: `let min = 0.5;` / `let max = 3.0;` at `draw_tiled:96-97`. These define the target screen-space dot-spacing range that `wrap_scale_multiplier` normalizes into. Same values appear as arguments in every test (`0.5, 3.0`).
- **Problem**: `min` / `max` are generic names for a specific visual tuning parameter. A reader has to trace where they're used to understand what they bound. Also: when they inevitably drift away from the test values, the tests won't catch it.
- **Alternative**:
  ```rust
  /// Target on-screen spacing range (in tile-widths) that the dot
  /// pattern wraps to as the user zooms.
  const MIN_WRAP_SPACING: f32 = 0.5;
  const MAX_WRAP_SPACING: f32 = 3.0;
  ```
  Tests reference the constants instead of duplicating `0.5, 3.0`.
- **Recommendation**: Do it.

## Considered and rejected

- **Invalidate the texture when `style.graph_background.dotted_radius_base` or `dotted_color` changes.** The texture is built once and never rebuilt, even if the style mutates. But `Style` is loaded from `style.toml` at startup and isn't mutated at runtime anywhere in the codebase. Adding invalidation for a scenario that doesn't happen is speculative.

- **Replace the hard-edged circle in `rebuild_texture` with anti-aliased falloff.** Would give smoother dots at display time, especially at fractional zoom multipliers. But the texture is 10–30 pixels with `TextureFilter::Linear` mag/min and mipmaps enabled, which already smooths out hard edges at most scales. Current output looks fine; not worth a correctness claim over aesthetics.

- **Eager-initialize the texture in a `new(gui: &mut Gui)` constructor.** Would drop the `Option` wrapper and the first-frame check. But `GraphUi` uses `#[derive(Default)]` throughout, and adding a non-default constructor solely for the background renderer propagates ergonomic ugliness upward. The lazy-init on first `render()` call is self-contained and cheap.

- **Use `OnceCell` / `LazyLock` for the texture.** Same idea as above. Simpler than `Option` + manual guard at the call site, but requires a `&mut Gui` at init — which a `OnceCell::get_or_init` closure can't receive without interior-mutability gymnastics. Not worth it.
