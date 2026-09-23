# Darkroom

A node-graph editor for image and data pipelines: a graph compiles → plans →
executes on a background worker, and the editor is a native desktop app on
Palantir.

## Crates

Workspace members, each after the crates it uses:

- **`common`** — typed ids, serialization, introspection, cancellation, file
  utilities. Pure leaf: nothing in-tree but its own `common-derive`.
- **`scenarium`** — the node-graph framework: authoring model plus the
  compile → plan → execute pipeline. Uses `common`.
- **`lumos`** — astronomical image-processing pipeline. Uses `common`,
  `imaginarium`, `fits-well`.
- **`lens`** — the node-function library: application-level nodes
  (filesystem watching, random generation) plus adapters from `imaginarium`
  and `lumos` into Scenarium's workflow. Uses `common`, `scenarium`,
  `imaginarium`, `lumos`.
- **`darkroom`** — the editor app and the only binary; editor work may need a
  matching Palantir change. `--features profile-with-tracy` forwards
  Palantir's Tracy zones. Uses `common`, `scenarium`, `lens`, `imaginarium`,
  `palantir`.

`default-members = ["darkroom"]`: a bare `cargo run` launches the editor; a
bare `cargo test` or `cargo clippy` covers `darkroom` alone.

Git submodules — `palantir` (immediate-mode GUI), `imaginarium` (CPU/GPU image
library), `fits-well` (FITS reader and writer), `quickbench` (micro-benchmark
harness, a `lumos` dev-dependency) — are standalone projects `exclude`d from
this workspace, each with its own lockfile and profiles. Run cargo for them
from their own directory: from the root, `cargo fmt -p` rejects them and
`clippy`/`test -p` build them under this workspace's lockfile. Changes inside
them, `Cargo.toml` above all, must stay valid in a standalone checkout; never
make them inherit from this workspace.

## Verification

Per touched member crate:

```
cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings && cargo test -p <crate> --tests --all-features
```

A crate's AGENTS.md may name its own test feature set, which replaces
`--all-features` in `cargo test` (`lumos` does). A submodule runs its chain
from its own directory.

## Conventions

**No backward compatibility.** Change serialized shapes and APIs freely; no
migrations, compat shims, legacy deserializers, or legacy-format tests.

**New ids come from `uuidgen`.** Every new `id_type!` UUID literal (`NodeId`,
`FuncId`, `TypeId`, …) is `uuidgen | tr 'A-Z' 'a-z'`, never hand-typed or
model-invented — those draw on no entropy and can collide. `rg` the new value
to confirm it is unique. A shipped id is the identity saved graphs bind to, so
it never changes.
