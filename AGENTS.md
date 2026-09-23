# Darkroom

A node-graph editor for image and data pipelines: a graph compiles → plans →
executes on a background worker, and the editor is a native desktop app on
Palantir.

## Crates

Workspace members, each listed after the crates it depends on:

- **`common`** — shared utilities: typed ids, cancellation, serialization,
  small extension traits. Pure leaf crate — its only in-tree dependency is its
  own `common-derive` proc-macro.
- **`scenarium`** — the node-graph framework: an authoring graph model plus a
  compile → plan → execute pipeline. Uses `common`.
- **`lumos`** — astronomical image-processing pipeline. Uses `common`,
  `imaginarium`, `fits-well`.
- **`lens`** — node-function library adapting image and astronomical
  processing into Scenarium's workflow. Uses `common`, `scenarium`,
  `imaginarium`, `lumos`.
- **`darkroom`** — the editor app, the workspace's only binary. Uses `common`,
  `scenarium`, `lens`, `imaginarium`, `palantir`.

Git submodules:

- **`palantir`** — our in-development immediate-mode GUI library.
- **`imaginarium`** — image library with CPU and GPU operations.
- **`fits-well`** — FITS reader and writer.
- **`quickbench`** — tiny no-frills micro-benchmark harness; a `lumos`
  dev-dependency.

`palantir`, `fits-well`, `imaginarium`, and `quickbench` are standalone
projects checked out here as git submodules and `exclude`d from this Cargo
workspace — each is its own workspace root, with its own lockfile and profiles.
Run cargo for them from inside their directory: from the root, `cargo fmt -p`
rejects them as non-members, and `clippy`/`test -p` build them as
dependencies under this workspace's lockfile and profiles. Changes inside them,
especially to `Cargo.toml`, must remain valid when the project is checked out
and built independently; do not make them inherit settings from the enclosing
workspace.

`default-members = ["darkroom"]`: a bare `cargo run` launches the editor, and a
bare `cargo test` or `cargo clippy` covers `darkroom` alone.

## Verification

For each touched member crate, one chained command:

```
cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings && cargo test -p <crate> --tests --all-features
```

A crate's own AGENTS.md may name a test feature set, which then replaces
`--all-features` in `cargo test` — `lumos` does. A submodule runs the same
chain from its own directory; `palantir` and `imaginarium` name their own
commands.

## Conventions

**Compatibility.** Existing project files and APIs do not need backward
compatibility for now. Change serialized shapes and break APIs freely when that
simplifies the current design; do not add migrations, compatibility shims,
legacy deserializers, or legacy-format tests.

**UUIDs / IDs.** Every new UUID literal (a `NodeId`, `FuncId`, `TypeId`, or
any other `id_type!`-backed id) must be generated with the real `uuidgen` tool,
lowercased — `uuidgen | tr 'A-Z' 'a-z'` — never hand-typed or model-invented.
Hand-made ids look unique but aren't drawn from any entropy source and risk
silently colliding with an existing id. After adding one, `rg` the new value
across the repo to confirm it's unique. These ids are the stable identity that
persisted graphs bind to, so once an id ships in a saved document it must not
change.
