AI coding rules for Rust projects.

## Project

Scenarium is a Cargo workspace for a node-based data processing pipeline
framework with a visual editor.

- **`common`** — shared utilities: typed ids, cancellation, serialization,
  small extension traits. Pure leaf crate — depends on nothing in-tree.
- **`scenarium`** — the node-graph framework: an authoring graph model plus a
  compile → plan → execute pipeline.
- **`darkroom`** — the editor app, built on Palantir.
- **`lens`** — node-function library adapting image and astronomical
  processing into Scenarium's workflow.
- **`lumos`** — astronomical image-processing pipeline.
- **`fits-well`** — FITS reader and writer.
- **`imaginarium`** — image library with CPU and GPU operations.
- **`quickbench`** — tiny no-frills micro-benchmark harness.
- **`palantir`** — our in-development immediate-mode GUI library.

`palantir`, `fits-well`, `imaginarium`, and `quickbench` are standalone
projects pulled into this workspace as git submodules. Changes inside them,
especially to `Cargo.toml`, must remain valid when the project is checked out
and built independently; do not make them inherit settings from the enclosing
workspace.

## Conventions

**Compatibility.** Existing project files and APIs do not need backward
compatibility for now. Change serialized shapes and break APIs freely when that
simplifies the current design; do not add migrations, compatibility shims,
legacy deserializers, or legacy-format tests.

**Watch the bench link count.** The root `[profile.bench]` is fat-LTO with
full debug info, so each bench binary links its entire dependency graph in
one codegen unit — GBs apiece — and cargo runs those links in parallel across
every target. A crate with many bench targets can exhaust RAM and get
OOM-killed on an unfiltered `cargo bench` / `cargo bench --no-run`.

- Compile-checking them: clippy `--all-targets` covers benches with no
  optimized link, which the verification chain already does.
- Running one: name the target — `cargo bench -p <crate> --bench <name>`.
  palantir keeps every criterion driver in one `criterion` target, so
  there the driver is a filter, not a target:
  `cargo bench -p palantir --bench criterion -- damage`. Its other three
  targets are the dhat allocation benches.
- Linking several: cap with `-j 2`.

**UUIDs / IDs.** Every new UUID literal (a `NodeId`, `FuncId`, `TypeId`, or
any other `id_type!`-backed id) must be generated with the real `uuidgen` tool,
lowercased — `uuidgen | tr 'A-Z' 'a-z'` — never hand-typed or model-invented.
Hand-made ids look unique but aren't drawn from any entropy source and risk
silently colliding with an existing id. After adding one, `rg` the new value
across the repo to confirm it's unique. These ids are the stable identity that
persisted graphs bind to, so once an id ships in a saved document it must not
change.
