# Layout system review

Focus: `prism/src/gui/graph_layout.rs` + `prism/src/gui/node_layout.rs` and their callers.

## Real design smells

### 1. Dragged nodes get laid out twice per frame

`graph_layout.rs:47` computes layout for every node including the dragged one (with offset from `gesture.node_drag_offset_for`); then `node_ui.rs:262` *re-runs* `layout.update(..)` on the same node. The reason: `gui.ui().interact(body_rect, ..)` at `node_ui.rs:215` needs `body_rect` (from the first pass) to read the drag delta that feeds the second pass. It works, but the ordering is subtle — a comment (`node_ui.rs:255-259`) has to explain why the second call is required.

The cleanest fix is to invert the order: interact with last frame's `body_rect`, accumulate the delta into `Gesture`, *then* run the single layout pass. That's exactly the "stale rect for interaction, fresh rect for rendering" pattern that egui itself uses internally. No correctness cost beyond one frame of pointer-follow lag — which egui already has everywhere else.

### 2. `GraphLayout.origin` is spread state

`graph_layout.rs:34` stores `origin` just so `node_ui.rs:262` can pass it back into the second `layout.update(..)`. If the double pass goes away, `origin` becomes a local in `update()` and the field disappears.

### 3. Port-position accessors silently compute garbage out-of-bounds

`node_layout.rs:73-107`. `input_center(999)` returns a real `Pos2` at `input_first_center.y + row_height*999`. Callers always index from the true port count, so it hasn't bitten, but a `debug_assert!` or bounds-checked indexing would cost nothing and catch drift bugs early.

### 4. `inited` flag is load-bearing for nothing

`node_layout.rs:20, 51, 126-128`. It guards `rebuild_port_galleys`, which already re-runs on `scale_changed`. On first frame, `scale_changed` is true (stored `1.0` vs actual scale), so the flag is never the sole reason for a rebuild. Drop it; replace with `self.input_galleys.is_empty() || scale_changed` or just always rebuild on first encounter.

## Possibly over-engineered, not urgent

### 5. `KeyIndexVec<NodeId, NodeLayout>`

Used only for keyed lookup + compact insert. The index position isn't read anywhere. A plain `HashMap<NodeId, NodeLayout>` plus a single-pass drain at end of `update()` would do the same job with one less abstraction.

### 6. Galley caches vs. layout rects are entangled

`NodeLayout` mixes things that are expensive to build (`Arc<Galley>`s) with things that are trivial (rects, centers). A cleaner split: `NodeGalleys` (invalidated on text/scale change) + per-frame computed `NodeRects` (pure function of galleys + pos + style). Removes the "is it stale?" question for the cheap half entirely.

## Alternative worth considering

**Make layout a pure function, cache only galleys.** Rects and port centers are computed from a handful of style values + galley sizes + node pos. Nothing there is expensive. If you computed them per-frame per-node inside `render_node`, `GraphLayout` shrinks to just a `HashMap<NodeId, NodeGalleys>`, and the double-update/origin-plumbing issues vanish because there's nothing to update out-of-band. Connection rendering does need port positions of *other* nodes, so you'd snapshot a `HashMap<NodeId, NodeRects>` during the node-render pass and read from it when drawing wires. That's the same amount of state as today, just built bottom-up instead of top-down.

## Not worth changing

- Port positions being recomputed per connection each frame — the math is literally one add; caching it would cost more than it saves.
- `PortInfo.center` being re-read per frame during drag — correct, because zoom/pan can change mid-drag.
- `rebuild_port_galleys` regenerating inputs+outputs+events together — func signature is immutable once assigned, so finer-grained invalidation is imaginary work.

## Highest-leverage change

Fix #1 (invert interact/update order) — it eliminates #2 for free and makes the whole flow linear: `handle_interactions → update all layouts once → render`. The rest is polish.
