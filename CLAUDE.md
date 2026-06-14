AI coding rules for Rust projects.

## Project state

Scenarium is a Cargo workspace for a node-based data processing pipeline framework with a visual editor. Workspace members:

- **`common`** — pure leaf crate of shared utilities: 2D buffers (`Buffer2`, bit-packed `BitBuffer2`), strongly-typed UUID IDs, serialization + file-format detection, async sync primitives (`Slot`, `PauseGate`, `ReadyState`), and small extension traits. Depended on by everything, depends on nothing in-tree.
- **`scenarium`** — the node-graph framework: an authoring graph model plus a compile→plan→execute pipeline that flattens composites and schedules async node functions on a tokio worker. Depends only on `common`.
- **`darkroom`** — the editor app and `default-member`; the Palantir-based UI (its own conventions in `darkroom/CLAUDE.md`).
- **`lens`** — image-processing function library that adapts `imaginarium` operations into `scenarium`'s node-based workflow.
- **`lumos`** — astronomical image-processing pipeline (RAW/FITS, starfield work).
- **`imaginarium`** — image library with CPU and wgpu GPU operations.
- **`quickbench`** — tiny no-frills micro-benchmark harness; benchmarks are `#[test] #[ignore]` fns run via `cargo test`.
- **`palantir`** — our in-development immediate-mode GUI library (see GUI-rewrite note below).

`default-members = ["darkroom"]`; only `.tmp` is `exclude`d. `imaginarium`, `quickbench`, and `palantir` are git submodules (see `.gitmodules`).

**`darkroom` + `palantir`.** `darkroom/` is the editor, built on **Palantir** — our own in-tree immediate-mode GUI library in `palantir/`. Palantir is a sibling project (workspace member + git submodule) with its own conventions in `palantir/CLAUDE.md`; changes to `darkroom/` may require coordinated changes in `palantir/`. Both are pre-1.0 and break freely.
