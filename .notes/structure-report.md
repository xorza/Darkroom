# Structural analysis — 2026-07-29

Generated with `cargo-modules 0.26`, `cargo-depgraph`, `cargo-workspace-unused-pub`,
`cargo-machete`. Raw graphs and SVGs in `.tmp/modules/`.

Scope: 278k lines of Rust, 9 crates.

## Crate graph — clean

```
darkroom ─┬─> palantir
          ├─> lens ──┬─> lumos ──┬─> imaginarium
          │          │           ├─> fits-well
          │          │           └─> common
          │          └─> scenarium ─> common
          ├─> scenarium
          ├─> imaginarium
          └─> common
```

Strict DAG, no cycles, correct layering. Nothing to fix at this level.

## Unused public surface — clean

`cargo workspace-unused-pub` flagged 102 items; 101 are `#[test]`/bench fns invoked
by a harness rather than by name. One real hit:

- `palantir/src/ui/harness/mod.rs:476` — `pub fn right_click_on` has no caller.

The workspace-level `unreachable_pub = "deny"` is doing its job. This axis needs no work.

## Unused dependencies

- `imaginarium/Cargo.toml:36` — `aligned-vec`, genuinely unreferenced.
- `lumos/Cargo.toml:22` — `libraw-rs-sys` is a **false positive**; the package's lib
  name is `libraw_sys`, which `io/raw/mod.rs:9` imports as `sys`.

## Module cycles

`cargo modules --acyclic` is unusable here: it evaluates the cycle check before
applying `--no-fns`/`--no-types`/`--no-owns`, so every crate trivially "fails" on a
type↔method pair. Cycles below come from parsing the filtered DOT (`uses` edges only)
and running Tarjan over it — see `.tmp/modules/analyze.py`.

| crate | modules | edges | largest SCC |
|---|---|---|---|
| common | 8 | 8 | — |
| lens | 22 | 40 | — |
| scenarium | 58 | 292 | **24** → **11** (see below) |
| lumos | 133 | 484 | **15** |
| darkroom | 103 | 452 | **43** |

`common` and `lens` are acyclic. The other three each have one dominant knot.

### Highest-leverage edges

For every intra-SCC edge, how many modules leave the SCC if it is removed:

**scenarium** (24-module SCC)

| gain | edge | actual import |
|---|---|---|
| −9 | `runtime::context` → `execution::outcome` | `context.rs:8` — only `LogEntry, LogLevel` |
| −6 | `execution::outcome` → `execution::cache::runtime` | `outcome.rs:2` — only `NodeRamUsage` |
| −2 | `execution::identity` → `graph` | |

Both top edges are single narrow type imports. `LogEntry`/`LogLevel` are logging
primitives sitting in `execution::outcome`; `NodeRamUsage` is a leaf value type sitting
in `execution::cache::runtime`.

**Done** — extracted to `execution/log.rs` and `execution/ram.rs`. Scenarium's SCC went
24 → 16; the whole `execution::cache::*` + `program` + `outcome` + `error` + `event`
cluster left the knot. A second, isolated 2-cycle (`program` ↔ `program::index`) is now
visible where it was previously buried inside the 24. Full workspace suite green.

The predicted −9/−6 assumed deleting the edges outright; the replacement modules import
`execution::identity`, which still reaches `graph`, so the realised drop is 8 rather
than 15. That residual is the next item.

**darkroom** (43-module SCC — essentially all of `gui`)

| gain | edge | actual import |
|---|---|---|
| −8 | `gui::app::editor` → `gui::main_window` | `editor/mod.rs:28` — only `MainWindow` |
| −3 | `gui` → `gui::app` | |
| −2 | `gui::scene` → `gui` | |

**lumos** (15-module SCC across `io` + `stacking`)

No single edge shrinks it by more than 1 — densely tangled rather than hinged on one
import. The layering inversion to fix first is `io/image/linear.rs:18`, where the
I/O-layer `linear` module imports `StackableImage` from `stacking::frame_store`.

### Cross-layer 2-cycles

- `darkroom::core::theme_pref` ↔ `darkroom::gui::theme` — `ThemeChoice` is declared in
  `core/theme_pref.rs` but its inherent `impl` block lives at `gui/theme.rs:302`.
  Moving the impl to the declaring module removes the only `core` → `gui` edge.
- `darkroom::core::document` ↔ `core::document::dock`
- `darkroom::core::script` ↔ `core::script::tcp`
- `scenarium::error` ↔ `scenarium::graph` — `error.rs` imports `GraphId`, `InputPort`,
  `NodeId`, `OutputPort`, `FuncId`; `graph/{mod,validate}.rs` import back from `error`.

## Coupling hotspots

Ranked by fan-in × fan-out.

**scenarium** — `graph` (19 in / 12 out), `execution::program` (11/12),
`node::definition` (21/5), `library` (12/6), `execution::cache::runtime` (7/10).

**darkroom** — `core::document` has **47 of 103** modules importing it; `gui::app`
(20/19), `gui::canvas` (11/25), `gui::scene` (25/4), `core::edit::intent::types` (29/2).

**lumos** — `io::image` (22/8), `io::image::linear` (17/7), `io::image::cfa` (13/9),
`stacking::combine::stack` (5/13).

`core::document` at 47 inbound is the single most concentrated dependency in the
workspace.

## Scenarium after the relocations

60 modules, 294 edges. Two cycles: **16** and **2**.

Re-scored leverage on the 16:

| gain | edge | actual import |
|---|---|---|
| −5 | `execution::identity` → `graph` | `identity.rs:16` — only `NodeId` |
| −2 | `graph` → `node::special` | `graph/mod.rs:16` — only `SpecialNode` |
| −1 | each of nine others | parent↔child `use` pairs |

### Extract `graph::address` — done

`graph/mod.rs` mixed two unrelated things: **address primitives** (`NodeId`, `OutputPort`,
`InputPort`, `Subscription` — pure value types) and **the authoring model** (`Graph`,
`GraphDef`, `Node`, `NodeKind`, `NodeRef`, `NodeSearch`, `CacheMode`). Everything reaching
into `graph` from the execution side wanted only the first group.

The four moved to `graph/address.rs`, a leaf with **zero outgoing edges** and 17 importers.
20 files repointed (no re-export shim, per the one-canonical-path rule) plus `lib.rs`.

**SCC 16 → 11.** `error` ↔ `graph` and `graph` ↔ `graph::interface` both gone. The knot is
now purely authoring — no `execution::*`, no `runtime::context`. `graph`'s fan-in dropped
19 → 12.

### Where scenarium stands

61 modules, 304 edges (edge count rises when a module splits; the cycle is the metric).

Largest SCC is 11: `graph` + `graph::{boundary,clone,interface,query,wiring}` + `library` +
`node::{definition,special}` + `error` + `elements::run_sinks`. Seven surviving 2-cycles,
five of them `graph` ↔ its own children.

Under the crate's no-`super::` rule those parent/child `use` pairs are close to structural,
so 11 is near the floor without splitting `Graph` itself. The two non-structural ones left
are `graph` ↔ `library` and `graph` ↔ `node::definition` — both real coupling
(`node::definition` has 20 inbound), but untangling them is a design question, not a move.

### Minor: `program` ↔ `program::index`

One import: `execution/program/index/mod.rs:10` takes `OutputRange` (a
`PoolRange<ExecutionOutput>` alias declared at `program/mod.rs:69`) for two method
signatures. Isolated and low value — noted for completeness.

## Suggested order

1. ~~Move `LogEntry`/`LogLevel` and `NodeRamUsage` out of their current modules.~~ **Done.**
2. ~~Extract `graph::address`.~~ **Done** — scenarium 24 → 11 overall.
3. Move `impl ThemeChoice` into `core::theme_pref` — removes darkroom's `core` → `gui` edge.
3. Move `MainWindow` out of `gui::main_window` or invert the `editor` dependency —
   darkroom SCC 43 → 35.
4. Fix `lumos` `io` → `stacking` inversion at `io/image/linear.rs:18`.
5. Drop `aligned-vec` from imaginarium; delete `palantir`'s `right_click_on`.

Not addressed: `core::document`'s 47 inbound edges. That is a real design question
(god-object vs. legitimately central document model), not a mechanical fix.
