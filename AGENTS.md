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
- **`darkroom`** — the editor and the only binary, a thin `main.rs` over the
  `darkroom` library; editor work may need a matching Palantir change. `--features profile-with-tracy` forwards
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

`x.py` at the root runs every check (`python x.py` on Windows). `gate` runs,
on the packages the working tree changed: rustfmt in place, clippy on every
target per lint set, plain clippy for the build that ships, rustdoc with
private items under `-D warnings`, the tests per test set, and the doc tests.
It stops after the first step that fails. Each cargo call prints one line, a
failure adds only its diagnostics or failed tests, and the full output is in
`target/x/last/`.

```
./x.py gate                        # the changed packages
./x.py gate -p <pkg>               # named packages; --workspace for every one
./x.py test -p <pkg> -- <filter>   # one step: fmt, lint, doc, test, nextest, doctest
./x.py test --lib                  # or --test <name>, in place of the set's targets
./x.py bench -- <filter>           # --smoke runs each case once, --no-run prints the binary
./x.py watch                       # rebuilds on change, so the cache stays warm
```

Check through `x.py`, not cargo: each call is one package with fixed flags,
so every run reuses the build `watch` keeps warm, and another flag set builds
another copy of the crate. A package's feature sets are in its `Cargo.toml`,
under `[package.metadata.x]`. `x.py` is one file, the same in darkroom and
in each submodule: a change to it is copied to the others.

A changed file in a submodule puts its packages in scope; they build in the
submodule's directory, into its target dir, under its own sets. `-p` names a
package of any root, and `--workspace` means this root's members.

The second clippy lints the build that ships: `--all-targets` turns `internals`
on through the self dev-dependency, so only a build of the library and binaries
alone shows what production compiles, such as a field only test code reads.
The doc step fails on a broken intra-doc link; it documents private items,
where most of the workspace's docs sit. Rustdoc never builds `cfg(test)`
code, so a doc names a test-only item in a code span, not a link.

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

## Benches

A crate with benches has one target, `benches/<crate>.rs`, whose one group runs
`bench::run`; each driver sits in a `bench.rs` beside the code it measures.
An `atomic` case times one operation on the unit production hands it; an
`integration` case times a pipeline stage, a frame or a public entry point end
to end on a scene. An id is `<kind>/<group>/<case>`, and criterion's filter
selects by it; a crate with GPU cases puts the backend first in every case, so
`'/cpu/'` opens no device. `./x.py bench` runs them in this workspace (lumos,
and darkroom's editor frame) and in each submodule, with that package's fixed
bench features; `--smoke` runs each case once, and a change to bench code runs
it.

## Conventions

- **No backward compatibility.** Change serialized shapes and APIs freely: no
  migrations, compat shims, legacy deserializers or legacy-format tests.
- **New ids come from `uuidgen | tr 'A-Z' 'a-z'`** — every new `id_type!`
  UUID (`NodeId`, `FuncId`, `TypeId`, …), never hand-typed or invented, which
  can collide; `rg` it to confirm it is unique. A shipped id never changes:
  saved graphs bind to it.
