# Darkroom

A node-graph editor for image and data pipelines: a graph compiles → plans →
executes on a background worker; the editor is a native desktop app on
Palantir.

## Crates

Workspace members, each after the crates it uses:

- **`common`** — typed ids, serialization, introspection, cancellation, file
  utilities. A leaf: in-tree it uses only its own `common-derive`.
- **`scenarium`** — the node-graph framework: the authoring model and the
  compile → plan → execute pipeline.
- **`lumos`** — the astronomical image-processing pipeline.
- **`lens`** — the node-function library: application nodes (filesystem
  watching, random generation) and adapters from `imaginarium` and `lumos`.
- **`darkroom`** — the editor and the only binary; editor work may need a
  matching Palantir change. `--features profile-with-tracy` forwards
  Palantir's Tracy zones.

`default-members = ["darkroom"]`: a bare `cargo run`, `test` or `clippy`
covers `darkroom` alone.

The submodules `palantir` (GUI), `imaginarium` (CPU/GPU images) and
`fits-well` (FITS) are standalone projects, `exclude`d from this workspace
with their own lockfiles and profiles. Run cargo for them from their own
directory, and keep their `Cargo.toml` valid in a standalone checkout —
never inherit from this workspace.

`libraw-sys/LibRaw` is upstream LibRaw at a release tag; never edit it.
Bindgen needs libclang: Xcode tools on macOS, `winget install LLVM.LLVM` on
Windows, `libclang-dev` on Linux.

## Verification

Per touched member crate:

```
cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings && cargo clippy -p <crate> -- -D warnings && RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps --document-private-items --all-features && cargo test -p <crate> --tests --all-features
```

The second clippy lints the build that ships: `--all-targets` turns `internals`
on through the self dev-dependency, so only a build of the library and binaries
alone shows what production compiles, such as a field only test code reads.
The doc step fails on a broken intra-doc link; it documents private items,
where most of the workspace's docs sit. Rustdoc never builds `cfg(test)`
code, so a doc names a test-only item in a code span, not a link. A crate's
AGENTS.md may replace `--all-features` in `cargo test` (`lumos` does).

## Test code

`internals` is the one feature for what only tests and benches build. A crate
with it dev-depends on itself with it, and every dev-dependency on such a
crate turns it on, so every test, bench and example build has it. Code a bench
or another crate's test reaches is gated `#[cfg(feature = "internals")]`; a
reach-in only the crate's own tests call is gated `#[cfg(test)]`. Never
`any(test, feature = "internals")`: a wrong gate shows as dead code in a
`--features internals` build. The one exception is a test-fixture tree, such
as lumos's `src/internals/`: it compiles whole under `internals`, and an
`expect(dead_code)` outside tests on its root covers what benches leave
unused. The submodules follow the same rule.

## Conventions

- **No backward compatibility.** Change serialized shapes and APIs freely: no
  migrations, compat shims, legacy deserializers or legacy-format tests.
- **New ids come from `uuidgen | tr 'A-Z' 'a-z'`** — every new `id_type!`
  UUID (`NodeId`, `FuncId`, `TypeId`, …), never hand-typed or invented, which
  can collide; `rg` it to confirm it is unique. A shipped id never changes:
  saved graphs bind to it.
