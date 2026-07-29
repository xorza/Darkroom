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
| scenarium | 58 | 292 | **24** |
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
in `execution::cache::runtime`. Relocating those two types to leaf modules takes the
SCC from 24 → 9 without touching any logic.

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

## Suggested order

1. Move `LogEntry`/`LogLevel` and `NodeRamUsage` out of their current modules —
   scenarium SCC 24 → 9, mechanical.
2. Move `impl ThemeChoice` into `core::theme_pref` — removes darkroom's `core` → `gui` edge.
3. Move `MainWindow` out of `gui::main_window` or invert the `editor` dependency —
   darkroom SCC 43 → 35.
4. Fix `lumos` `io` → `stacking` inversion at `io/image/linear.rs:18`.
5. Drop `aligned-vec` from imaginarium; delete `palantir`'s `right_click_on`.

Not addressed: `core::document`'s 47 inbound edges. That is a real design question
(god-object vs. legitimately central document model), not a mechanical fix.
