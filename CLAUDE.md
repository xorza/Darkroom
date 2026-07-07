AI coding rules for Rust projects.

## Project state

Scenarium is a Cargo workspace for a node-based data processing pipeline framework with a visual editor. Workspace members:

- **`common`** — pure leaf crate of shared utilities: 2D buffers (`Buffer2`, bit-packed `BitBuffer2`), strongly-typed UUID IDs, serialization + file-format detection, async sync primitives (`Slot`, `PauseGate`, `ReadyState`), and small extension traits. Depended on by everything, depends on nothing in-tree.
- **`scenarium`** — the node-graph framework: an authoring graph model plus a compile→plan→execute pipeline that flattens composites and schedules async node functions on a tokio worker. Depends only on `common`.
- **`darkroom`** — the editor app and `default-member`; the Aperture-based UI (its own conventions in `darkroom/CLAUDE.md`).
- **`lens`** — image-processing function library that adapts `imaginarium` operations into `scenarium`'s node-based workflow.
- **`lumos`** — astronomical image-processing pipeline (RAW/FITS, starfield work).
- **`imaginarium`** — image library with CPU and wgpu GPU operations.
- **`quickbench`** — tiny no-frills micro-benchmark harness; benchmarks are `#[test] #[ignore]` fns run via `cargo test`.
- **`aperture`** — our in-development immediate-mode GUI library (see GUI-rewrite note below).

`default-members = ["darkroom"]`; only `.tmp` is `exclude`d. `imaginarium`, `quickbench`, and `aperture` are git submodules (see `.gitmodules`).

**`darkroom` + `aperture`.** `darkroom/` is the editor, built on **Aperture** — our own in-tree immediate-mode GUI library in `aperture/`. Aperture is a sibling project (workspace member + git submodule) with its own conventions in `aperture/CLAUDE.md`; changes to `darkroom/` may require coordinated changes in `aperture/`. Both are pre-1.0 and break freely.

## Dev tools (`tools/`)

- **`tools/rust-outline`** — generates a Markdown structural outline of Rust source (type defs with fields/types/visibility, `impl`/`trait` method signatures). Parses the real AST via `syn` and renders through `prettyplease`. Standalone: its own empty `[workspace]` keeps it out of the root `cargo build`, so it's neither a member nor `exclude`d. Run `tools/rust-outline/outline <PATH> [-o OUT.md] [--tests]` (respects `.gitignore`, skips `#[cfg(test)]` by default). Conventions in `tools/rust-outline/CLAUDE.md`. Useful for handing an agent a whole crate's shape without dumping every file.

## Conventions

**UUIDs / IDs.** Every new UUID literal (a `TypeId`, `FuncId`, `SubgraphId`, or any other `id_type!`-backed id) must be generated with the real `uuidgen` tool, lowercased — `uuidgen | tr 'A-Z' 'a-z'` — never hand-typed or model-invented. Hand-made ids look unique but aren't drawn from any entropy source and risk silently colliding with an existing id. After adding one, `rg` the new value across the repo to confirm it's unique. These ids are the stable identity that persisted graphs bind to, so once an id ships in a saved document it must not change.
