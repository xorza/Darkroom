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

`layout` + 11 of its submodules + `scene`, `scene::tree`, `scene::shapes`.

**No cheap fix.** The best single edge removal is worth −2, and there are three of them:

| gain | edge | import |
|---|---|---|
| −2 | `layout` → `scene::tree` | `layout/mod.rs:22` — `use crate::scene::tree::Tree;` |
| −2 | `scene::tree` → `layout::scrollbars` | `scene/tree/mod.rs:29` — `use crate::layout::scrollbars::ScrollBarsDef;` |
| −2 | `layout::scrollbars` → `layout::engine` | `layout/scrollbars/mod.rs:24` — `use crate::layout::engine::LayoutEngine;` |

This is a *densely tangled* knot, not a hinged one — the lumos shape, not the scenarium
shape. Scenarium had single imports worth −9/−6/−5; nothing here comes close. Layout and
scene are mutually recursive by design (layout reads the tree, the tree carries layout
definitions), so this may be inherent rather than accidental.

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

## Actionable items

Ranked honestly. Palantir has no scenarium-style cheap wins.

1. **Decide whether the `internals` graph matters.** If yes, `ui::bench_fixture`'s 42
   outbound edges are the single biggest structural lever in the crate. If no, ignore
   tiers below 3 entirely.
2. **`layout::scrollbars` → `layout::engine`** (`layout/scrollbars/mod.rs:24`) — the only
   one of the three −2 edges that is *intra*-`layout`, so it does not cross a subsystem
   boundary and is the least likely to be inherent. Cheapest thing to try.
3. **Leave `layout` ↔ `scene` alone** unless you already believe it is wrong. The graph
   says tangled, not misplaced, and no mechanical move fixes it.

### Checked and rejected

`ShapedText` (declared `layout/mod.rs:73`, imported by `text/system.rs:22`) looked like a
text type parked in `layout`. It is not — `layout/mod.rs:39` stores `text_shapes:
Vec<ShapedText>` on the layout result and `layout/cache/mod.rs` uses it in four places.
It is a legitimate shared type at the layout/text boundary. No action.

## Methodology caveat

**cargo-modules counts intra-doc links as `uses` edges.** 13 of 1765 (0.7%) are
documentation references with no code behind them. Small, but not harmless: one reported
2-cycle, `primitives::stroke` ↔ `scene::shapes::paint`, is *entirely* a doc-comment
artifact and does not exist in code. All numbers above have the 13 removed.

Every edge cited in the actionable list was read in source before being reported.
