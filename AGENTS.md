# Darkroom

A node-graph editor for image and data pipelines: a graph compiles → plans →
executes on a background worker; the editor is a native desktop app on
Palantir.

## Crates

Workspace members, each after the crates it uses:

- **`common`** — the shared base: ids, serialization, cancellation, the 2D
  grid, and every I/O and thread call. A leaf: in-tree it uses
  only its own derive crate.
- **`simd`** — every vector instruction of the workspace: the vector types,
  their ops specified to the bit, and the kernel dispatch. A leaf.
  `simd/AGENTS.md` holds its decisions.
- **`scenarium`** — the node-graph framework: the authoring model and the
  compile → plan → execute pipeline.
- **`imaginarium`** — pixels in both layouts, interleaved and planar, with
  their codecs and ops, on the CPU and, behind a feature, the GPU.
- **`libraw`** — a safe wrapper over LibRaw: LibRaw built statically from its
  submodule, every `unsafe` call into it, and LibRaw's state read as plain
  Rust values.
- **`lumos`** — the astronomical image-processing pipeline.
- **`lens`** — the node-function library: application nodes and adapters from
  `imaginarium` and `lumos`.
- **`darkroom`** — the editor's library: its core, its GUI and the driver that
  plays input into it; editor work may need a matching Palantir change.
  `darkroom/AGENTS.md` holds its decisions, the editor harness, the document
  file and wire ties among them.
- **`darkroom-app`** (`app/`) — the `darkroom` binary: the window, and with
  `--mcp --headless` the editor as an MCP server over stdio, for an agent to
  open projects in, click through and look at.

The submodules `palantir` (GUI) and `fits-well` (FITS) are standalone
projects, `exclude`d from this workspace with their own lockfiles and
profiles. Run cargo for them from their own directory, and keep their
`Cargo.toml` valid in a standalone checkout — never inherit from this
workspace.

`libraw/LibRaw` is upstream LibRaw at a release tag; never edit it.
Bindgen needs libclang: Xcode tools on macOS, `winget install LLVM.LLVM` on
Windows, `libclang-dev` on Linux.

## Verification

`cargo drive` runs every check. The driver is `cargo-drive`, installed with
`cargo install --locked --git https://github.com/xorza/drive`; the drive table
in the root `Cargo.toml` names the version the project needs, and another
version refuses to run. Run it from the root.

```
cargo drive check              # after each change: fmt, then the lints
cargo drive test [args]        # the tests; the runner's arguments pick some
cargo drive bench [args]       # the benches; a filter, or --test to run each case once
cargo drive gate               # the full check, as CI runs it with --ci
cargo drive run <name> [args]  # a command of the root's own, from its drive table
```

An agent sees the editor through the `darkroom` MCP server: `cargo drive run mcp`
builds it, and Claude Code starts it, `darkroom --mcp --headless`, from
`.mcp.json` on its next launch. Then
`darkroom_open {"project": "darkroom/sample/sample.darkroom", "copy": true}` and
`darkroom_shot {}`. A project opens in place, so leave out `copy` only to edit the
file itself.

## Test code

`internals` is the one feature for what only tests and benches build. A crate
with it dev-depends on itself with it, and every dev-dependency on such a
crate turns it on, so every test, bench and example build has it. Code a bench
or another crate's test reaches is gated `#[cfg(feature = "internals")]`; a
reach-in only the crate's own tests call is gated `#[cfg(test)]`. Never
`any(test, feature = "internals")`: a wrong gate shows as dead code in a
`--features internals` build. The one exception is a test-fixture tree: it
compiles whole under `internals`, and an `expect(dead_code)` outside tests on
its root covers what benches leave unused. The submodules follow the same
rule.

## Benches

A crate with benches has one target, `benches/<crate>.rs`, whose one group
runs `bench::run`; each driver sits in a `bench.rs` beside the code it
measures. An `atomic` case times one operation on the unit production hands
it; an `integration` case times a pipeline stage, a frame or a public entry
point end to end on a scene. An id is `<kind>/<group>/<case>`, and criterion's
filter selects by it; a crate with GPU cases puts the backend first in every
case, so `'/cpu/'` opens no device. An `atomic` case of a vector kernel runs
for the scalar reference and each tier the CPU has. `cargo drive bench` runs
the benches of the root it runs in; each submodule runs its own.

## Conventions

- **I/O and thread creation go through `common`**; `clippy.toml` refuses the
  direct calls elsewhere, tests included. A task on a running pool or runtime
  is not a thread and stays as it is.
- **A library takes bytes, readers and writers, never paths**; the caller
  opens through `common::fs`. The submodules and imaginarium's codecs are
  such libraries.
- **Parallel loops do not nest.** `common::par` refuses a loop started inside
  another: rayon has no work isolation, so a worker that waits in an inner loop
  runs other jobs on its stack, and a per-thread scratch is not its own.
  `common::par`'s module doc gives the sources.
- **Arch code lives in `simd` alone.** No other crate holds `std::arch`,
  `#[target_feature]` or a `cfg(target_arch)`; `clippy.toml` refuses the
  register types elsewhere. A kernel is generic over `simd::Isa`.
- **Every vector kernel is bit-identical to its scalar reference**, and a test
  holds each tier the CPU has to it, the special values of its inputs
  included.
- **`From` and `TryFrom` are lossless.** A conversion that loses data is a
  named method each caller names, as `InterleavedImage::without_alpha`.
- **Errors are cases, each in its domain**, a foreign error kept as its own
  type. A type valid by construction has a `new` that panics and a `checked`
  that returns the error. Refused: one `Error` of messages, which no caller
  can match on.
- **No backward compatibility.** Change serialized shapes and APIs freely: no
  migrations, compat shims, legacy deserializers or legacy-format tests.
- **New ids come from `uuidgen | tr 'A-Z' 'a-z'`** — every new `common::Id`
  UUID, never hand-typed or invented, which can collide; `rg` it to confirm it
  is unique. A shipped id never changes: saved graphs bind to it.
- **Decisions live in the `AGENTS.md` files and in doc comments**, each with
  its source: a link, a measurement, the practice it follows. Refused: a
  research folder of dated facts, which the code outgrows and no one reads
  again.
- **A document is parsed once, at load.** The load handles each inconsistency
  with the library and reports it; the edit gate keeps the document
  consistent after that. At run time and in each frame, code asserts the
  document is valid and does not check it again or fall back.
- **Raw bytes are lowercase hex in text.** A serialized field of bytes, a
  digest or a byte string, takes `#[serde(with = "common::hex_bytes")]`: a
  text format writes two hex digits a byte, as blake3, git and `Cargo.lock`
  write a digest, and a binary format the bytes as the type holds them, so a
  binary layout does not move. Refused: serde's own form, a tuple of numbers
  four times as long in a document; base64, a third shorter, which mixes case
  and reads worse.

## Names

One word, one meaning, in the workspace crates; the submodules keep their own.
`cargo drive vocabulary` lists every declared name and word for a review.

- **A wire joins an output to an input.** `Binding` is what an input holds,
  `Const` or `Wire`; the GUI draws a wire and an event subscription alike as
  wires. Refused: edge, connection and link for it — "edge" is the graph's
  general word for a wire or a subscription, and "link" a text hyperlink.
- **Fixed short forms.** `idx`, `pos`, `ctx`/`Ctx`, `dir`, `bg`, `col`,
  `exec`, `desc`, `cur`, `prev`, `wid` (a palantir `WidgetId`), and their
  plurals with `s` (`idxs`, `poss`), in every name; prose and test names keep
  whole words. A name a foreign API sets keeps its own form, as
  `Iterator::position` and `std::task::Context`; a term of another meaning
  keeps its own, as a sensor's dark current and palantir's `ContextMenu`.
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
  `imaginarium::AffineWarp` beside `lumos::Transform`; an error and its alias
  carry their domain, as `ExecError` and `ExecResult`.
- **One channel is `mono`.** `luma` is the Rec. 709 weighted sum of encoded
  samples, and an RGB value with `R = G = B` a neutral. Refused: `L`, the PIL
  mode name; `gray` and `grey`, which the crates spelled both ways.
- **A descriptor is a `desc`, a `USize2` a `size`.** Refused: `dimensions`,
  which named both.
- **Bytes are KiB, MiB and GiB**, 1024-based as RAM is sized, the IEC 80000-13 names. Refused:
  SI kB at 1000, and KB at 1024, which the standard reads as 1000
  ([Ubuntu units policy](https://wiki.ubuntu.com/UnitsPolicy)).
- **A run time under 1 µs reads in ns**, and one above 0 that prints as `0ns` reads `<1ns`, so
  time taken never reads as none.
- **The surround is the editor's menu bar, tab strip and status bar**;
  `chrome` is palantir's `Chrome` alone, a widget's fill, border, radii and
  shadow. Refused: chrome for the surround, and frame, which is one pass of the
  render loop.
