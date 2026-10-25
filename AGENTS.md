# Darkroom

A node-graph editor for image and data pipelines: a graph compiles → plans →
executes on a background worker; the editor is a native desktop app on
Palantir.

## Crates

Workspace members, each after the crates it uses:

- **`common`** — typed ids, serialization, introspection, cancellation, the
  2D grid, CPU features, I/O and thread creation. A leaf: in-tree it uses only its own `common-derive`.
- **`scenarium`** — the node-graph framework: the authoring model and the
  compile → plan → execute pipeline.
- **`imaginarium`** — interleaved images in nine pixel formats: their codecs,
  SIMD and GPU ops. GPU work is behind its `wgpu` feature.
- **`lumos`** — the astronomical image-processing pipeline.
- **`lens`** — the node-function library: application nodes (filesystem
  watching, random generation) and adapters from `imaginarium` and `lumos`.
- **`darkroom`** — the editor and the only binary, a thin `main.rs` over the
  `darkroom` library; editor work may need a matching Palantir change. `--features profile-with-tracy` forwards
  Palantir's Tracy zones.

`default-members = ["darkroom"]`: a bare `cargo run`, `test` or `clippy`
covers `darkroom` alone.

The submodules `palantir` (GUI) and `fits-well` (FITS) are standalone
projects, `exclude`d from this workspace with their own lockfiles and profiles. Run cargo for them from their own
directory, and keep their `Cargo.toml` valid in a standalone checkout —
never inherit from this workspace.

`libraw-sys/LibRaw` is upstream LibRaw at a release tag; never edit it.
Bindgen needs libclang: Xcode tools on macOS, `winget install LLVM.LLVM` on
Windows, `libclang-dev` on Linux.

## Verification

`cargo drive` runs every check: the driver is `cargo-drive`, the same in every
project, installed with `cargo install --locked --git
https://github.com/xorza/drive`. The drive table's `version` names the driver
the project needs, and an older or newer one refuses to run. Run it from the
root.

```
cargo drive check          # after each change: fmt, then clippy --all-targets --all-features
cargo drive test [args]    # the tests, each time; the runner's arguments pick some: a test name
cargo drive bench [args]   # the benches; a filter, or --test to run each case once
cargo drive gate           # the full check: fmt, each lint set, ship, tests, docs; CI adds --ci
```

`check` takes no arguments: each lane covers the whole workspace, and the
workspace of each path dependency outside it, in its own folder. A step does
not run again when no input changed since it passed; under nextest, the tests
run only for the crates a change reaches and every crate that depends on them.
Each step prints one line; a failure prints each diagnostic once and names its
log, `target/drive/log/<step>.log`, which holds no passing test. A run ends on
one verdict line: `ok`, or `FAIL` with the failed steps and the first error,
and `target/drive/last-<command>.txt`, which holds all the run printed; above
it, each failed step shows its first lines again. So `| tail -1` is enough: on
a failure, read the file it names rather than run the command again. Never
run cargo by hand: another shape of a cargo command builds another copy of
the crates.
`check` and `gate` format the code and end at the first failed step; `gate
--ci` formats nothing and runs every step.

A root's flags are in its `Cargo.toml`, under `[workspace.metadata.drive]`,
or `[package.metadata.drive]` in a root that is one package. `gate` lints with
each set of the `lint` key and tests with each set of the `test` key, both
`--all-features` by default; `check` lints with `--all-features` alone.
`docs = true` adds rustdoc and the doc tests to `gate`, after the others; they
are off by default. `test-runner` is `nextest` (the default) or `cargo-test`,
which needs nothing installed but runs every test when a change reaches any
crate. Every lane runs `--locked`: after a change to the dependencies,
`cargo update --workspace` writes the lock.

The ship lane lints the build that ships: `--all-targets` turns `internals`
on through the self dev-dependency, so only a build of the library and binaries
alone shows what production compiles, such as a field only test code reads.
Rustdoc never builds `cfg(test)` code, so a doc names a test-only item in a
code span, not a link.

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
`'/cpu/'` opens no device. `cargo drive bench` runs the benches of the root
it runs in (here imaginarium, lumos, and darkroom's editor frame; each
submodule runs its own), with the root's fixed bench features, `wgpu` among
them; `--test` runs each case once, and a change to bench code runs it.

## Conventions

- **I/O and thread creation go through `common`** (`fs`, `env`, `process`,
  `thread`, `task`, `par`, `BgRuntime`); `clippy.toml` refuses the direct calls
  elsewhere, tests included. Submodules get bytes or readers, never paths. A
  task on a running pool or runtime is not a thread and stays as it is.
- **No backward compatibility.** Change serialized shapes and APIs freely: no
  migrations, compat shims, legacy deserializers or legacy-format tests.
- **New ids come from `uuidgen | tr 'A-Z' 'a-z'`** — every new `common::Id`
  UUID (`NodeId`, `FuncId`, `TypeId`, …), never hand-typed or invented, which
  can collide; `rg` it to confirm it is unique. A shipped id never changes:
  saved graphs bind to it.
- **A document is parsed once, at load.** The load handles each inconsistency
  with the library and reports it; the edit gate keeps the document
  consistent after that. At run time and in each frame, code asserts the
  document is valid and does not check it again or fall back.

## Names

One word, one meaning, in the workspace crates; the submodules keep their own.
`cargo drive vocabulary` lists every declared name and word for a review.

- **A wire joins an output to an input.** `Binding` is what an input holds,
  `Const` or `Wire`; the GUI draws a wire and an event subscription alike as
  wires. Refused: edge, connection and link for it — "edge" is the graph's
  general word for a wire or a subscription, and "link" a text hyperlink.
- **Fixed short forms.** `idx`, `pos`, `ctx`/`Ctx`, `dir`, `bg`, `col`,
  `exec`, `desc`, `cur`, `prev`, `wid` (a palantir `WidgetId`), and their
  plurals with `s` (`idxs`, `poss`),
  in every name; prose and test names keep whole words. A name a foreign API
  sets keeps its own form, as `Iterator::position` and `std::task::Context`;
  a term of another meaning keeps its own, as a sensor's dark current and
  palantir's `ContextMenu`.
- **`count` is a count, `len` a count of elements or bytes, `length` a
  geometric length.** `*_count`, as std's `Iterator::count`; `len` as std's
  `len()`. Refused: `num_*` and `n_*` for a count — `SmallN` is the statistics
  term — and `length` for bytes.
- **`*Config` is what a caller sets, `*Params` what a stage derives** from a
  config and reads. Refused: `*Options` and `*Settings`.
- **An acronym is one word,** as the Rust API Guidelines case it: `GraphUi`,
  `LmModel`.
- **A type name keeps one meaning in the workspace.** Where two crates would
  declare one name past its module, the narrower use takes a qualified one, as
  `DrawablePreview` beside `lumos::PreviewImage`; an error and its alias carry
  their domain, as `ExecError` and `ExecResult`.
- **The surround is the editor's menu bar, tab strip and status bar**, as
  `AppColors::surround_fill`; `chrome` is palantir's `Chrome` alone, a
  widget's fill, border, radii and shadow. Refused: chrome for the surround,
  and frame, which is one pass of the render loop.
