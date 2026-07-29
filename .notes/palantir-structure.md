# Palantir structure — 2026-07-29

`cargo modules dependencies --lib -p palantir --no-owns --no-fns --no-traits --no-types
--no-externs --no-sysroot`, then SCC + feedback-edge scoring (`.tmp/modules/analyze.py`).

260 modules, 1765 edges — the largest crate in the workspace by module count.

## The headline: the big cycle is mostly test-support

Raw graph: **10 cycles, largest 34 modules**, and the top edges look dramatic (−21, −19).

Excluding `internals`-gated modules (`*::bench`, `*::internals`, `ui::bench_fixture`),
the picture changes completely:

| | cycles | largest SCC | best single edge |
|---|---|---|---|
| raw | 10 | **34** | −21 |
| production only | 10 | **15** | **−2** |

Those modules are correctly gated — `palantir/src/ui/mod.rs:1` and `lib.rs:83` both carry
`#[cfg(feature = "internals")]`. So the 34-module knot exists only in `--features internals`
builds; a default build's worst cycle is 15.

`ui::bench_fixture` alone reaches **42** modules. If tightening the `internals` build matters,
that fixture is the lever. If it does not, this whole tier is noise.

## Production knot: `layout` ↔ `scene` (15 modules)

> **Correction.** An earlier revision of this file called this knot "densely tangled, no
> cheap fix, best edge −2." That was a **bug in the scoring**, not a property of the code:
> the scorer compared each cut against the *globally* largest SCC rather than against the
> component the cut edge belongs to. Because an unrelated 13-module `widgets` cycle exists,
> every layout cut appeared capped at 15 − 13 = 2. Scored per-component, the top edge is
> worth **−4**. Scenarium's numbers were unaffected — its only other cycle is 2 modules, so
> the global max was always the component of interest.

### Actual structure

The 15 decomposes cleanly:

- **11 modules** — `layout::{engine, cache, canvas, grid, intrinsic, scroll, scrollbars,
  stack, support, wrapstack, zstack}`, a driver↔engine mesh. Inherent (see below).
- **`layout`** (the parent module) — in the SCC only via a path through `scene`.
- **`scene`, `scene::tree`** — attached by exactly **one** back-edge.
- **`text::system`** — attached by exactly **one** back-edge.

Everything else is one-directional: 11 driver modules import `scene::tree`, and nothing
in `scene` imports them back except that single edge.

### The two back-edges, scored per component

| gain | edge | import |
|---|---|---|
| **−4** | `scene::tree` → `layout::scrollbars` | `scene/tree/mod.rs:29` — `use crate::layout::scrollbars::ScrollBarsDef;` |
| −1 | `text::system` → `layout` | `text/system.rs:22` — `use crate::layout::ShapedText;` |

Cutting the first removes `layout`, `scene`, `scene::tree` **and** `text::system` — the
last because its only route into the cycle ran through `layout`. So the second edge is
structurally redundant once the first is fixed.

Remaining production cycles: 13 (`app`/`ui`/`widgets`/`widgets::theme::*`), 8 (`text::*`),
5, 4, 3, 3, 3, 2, 2.

## Coupling hotspots

| in | out | module |
|---|---|---|
| 47 | 47 | `ui` (1334 lines) |
| 58 | 5 | `primitives::rect` |
| 41 | 15 | `scene::node` |
| 21 | 21 | `shape` |
| 16 | 24 | `scene::tree` |
| 11 | 26 | `layout::engine` |
| 9 | 26 | `widgets::theme` |
| 4 | 42 | `ui::bench_fixture` *(gated)* |

`primitives::rect` at 58 inbound / 5 outbound is the *good* shape — a widely-used leaf.
`ui` at 47/47 is the immediate-mode context; central by design, but it is both the most
depended-on and most depending module in the crate, which is worth watching.

## The fix: move `ScrollBarsDef` into `layout::types`

**`GridDef` is the exact precedent.** The two are structurally identical — a
record pushed onto the `Tree` at record time and read by a driver during layout — but
they live in different places:

| | definition | id | on the tree | cycle? |
|---|---|---|---|---|
| grid | `layout/types/track.rs:94` | `layout::types::layout_mode` | `scene/tree/mod.rs:96` | no |
| scrollbars | **`layout/scrollbars/mod.rs:51`** | `layout::types::layout_mode` | `scene/tree/mod.rs:99` | **yes** |

`ScrollBarsDefId` already lives in the leaf vocabulary module. Only the `ScrollBarsDef`
struct was left behind in the driver, and that is the whole cycle.

### Feasibility — verified

The struct's fields need only `NodeId`, `Vec2`, `BVec2`, `Spacing`. The driver module's
other imports (`LayerLayout`, `Axis`, `LayoutEngine`, `Tree`, `Rect`, `Size`,
`InternedText`) belong to its *functions*, not the struct, and stay put.

- `scene/tree/node.rs` (home of `NodeId`) imports only `primitives::*` and
  `scene::node::columns` — it cannot reach `layout`, so no new cycle.
- `layout/types/layout_mode.rs:3` already imports `crate::scene::visibility::Visibility`,
  so `layout::types` → `scene::*` is established precedent.

Three call sites move: `widgets/scroll/mod.rs:697` (constructs), `scene/tree/mod.rs:29`
(stores), `layout/scrollbars/mod.rs:131,196` (reads). The `impl ScrollBarsDef` visual-hash
block moves with the struct.

**Result: layout SCC 15 → 11**, and `layout`, `scene`, `scene::tree`, `text::system` all
leave. Simulated on the graph; the residual 11 is the driver↔engine mesh below.

## What stays, and why

The remaining 11 is `layout::engine` ↔ the nine drivers ↔ `layout::cache`/`support`/
`intrinsic`. This is **genuine mutual recursion**, not misplacement:

`LayoutEngine` (`layout/engine.rs:201`) is `{ scratch, cache_rebuild, text, cache }` — a
recursion *context*. The engine dispatches by `LayoutMode` to a driver; the driver
recurses into its children by calling back through the engine (`layout: &mut LayoutEngine`
at `stack/mod.rs:117,158,282,395`). A tree-walking layout algorithm with pluggable
drivers has this shape by construction.

Breaking it would mean splitting the context from the dispatcher and threading a
recursion callback through every driver signature — `stack/mod.rs:122-123` already shows
that pattern with `impl FnMut(&mut LayoutEngine, NodeId)` closures, so it is *possible*
and would stay monomorphized. But it rewrites the crate's hottest path for module-graph
tidiness inside a single subsystem. **Not worth it.** Recommend leaving.

## Optional, independent of the above

`ShapedText` (`layout/mod.rs:73`) is `{ measured: Size, key: TextShapeKey }` — produced by
`text::system`, stored by `layout`. Once `ScrollBarsDef` moves, this edge buys **no**
structural improvement. It is still arguably misfiled (a two-field handoff record forcing
`text` → `layout`), and `layout/types/` would take it — `text/key.rs` imports
`layout::types::align` already, so the direction is established. Purely cosmetic; do it
only if the boundary bothers you.

An earlier revision of this file rejected the `ShapedText` finding outright on the grounds
that `layout/mod.rs:39` and `layout/cache/mod.rs` consume it. Those consumers are real, but
they argue for a *leaf* home, not for the current one.

## Actionable items

1. **Move `ScrollBarsDef` to `layout/types/`**, mirroring `GridDef`. Three call sites.
   Layout SCC 15 → 11. This is the whole fix.
2. **Decide whether the `internals` graph matters.** If yes, `ui::bench_fixture`'s 42
   outbound edges are the biggest remaining lever. If no, ignore that tier.
3. **Leave the driver↔engine mesh alone.**

## Methodology caveat

**cargo-modules counts intra-doc links as `uses` edges.** 13 of 1765 (0.7%) are
documentation references with no code behind them. Small, but not harmless: one reported
2-cycle, `primitives::stroke` ↔ `scene::shapes::paint`, is *entirely* a doc-comment
artifact and does not exist in code. All numbers above have the 13 removed.

Every edge cited in the actionable list was read in source before being reported.
