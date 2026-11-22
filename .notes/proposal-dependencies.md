# Proposal: dependencies Darkroom can drop

Temporary: each item that lands folds its decision into the `AGENTS.md` of the crate it governs,
and this file goes.

The question for each third-party crate a workspace crate names: would writing the part we use
take crates out of the build? Counted are the crates that actually build (`cargo tree` per
target, the workspace's default features), not the lockfile, which also lists optional
dependencies nothing turns on. palantir and fits-well are their own projects and left out, except
where noted. 2026-11-22, Darkroom at `9a386bddc`.

The build compiles 311 third-party crates on macOS, 369 on Linux and 306 on Windows.

## Candidates

| Crate | Crates gone (macOS / Linux / Windows) | What we use | Recommendation |
|---|---|---|---|
| `rmcp` | 27 / 26 / 26 | the MCP server: 5 files of `app/src/mcp`, its params' `JsonSchema` derives | Decide: own JSON-RPC server |
| `bindgen` (build) | 8 / 8 / 8, and libclang on every machine | LibRaw's C API, regenerated per build | Do: commit the bindings |
| `tracing-subscriber` | 7 / 7 / 7; 17 with `bindgen` (the `regex` stack) | `RUST_LOG` filter, stderr lines, the driver's `LogRing` | Do: one subscriber of our own |
| `nalgebra` | 8 / 9 / 9 | lumos's least squares, DLT and TPS: QR, SVD, dense solves | Defer |
| `sysinfo` | 2 / 1 / 3, and a heavy compile | free memory, this process's memory, a path's file system | Do: the OS calls in `common::platform` |

`strum`, `clap` and `rand` have landed (11 crates). The items marked Do take about 19 more out of
each build on macOS; with `rmcp`, about 46.

### `rmcp`: an MCP server of our own

- **Uses**: `app/src/mcp` serves six tools (`darkroom_open`, `_act`, `_state`, `_ui`, `_shot`,
  `_wait_run`) over stdio through `initialize`, `tools/list` and `tools/call`. The tests drive it
  through rmcp's client.
- **Gone**: rmcp, its macros, `schemars` with `darling`, `chrono`, `sha2` and the digest crates,
  `futures`. `serde_json` stays: the protocol is JSON.
- **Replacement**: a JSON-RPC 2.0 loop over newline-delimited stdio, as the MCP stdio transport
  specifies
  ([transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports));
  each tool's input schema derived by `common-derive`, beside the derives it already holds, or
  written beside its params struct; the tests speak JSON-RPC to the server directly. About 500 lines with tests.
- **Cost and risk**: the protocol is ours to follow as it moves (2025-06-18 now); capability
  negotiation, cancellation and progress notifications are what rmcp gives that we would
  have to add when a client needs them. The 27 crates are the largest single saving here.

### `bindgen`: bindings committed per target

- **Uses**: `libraw/build.rs` runs bindgen over `libraw.h` on every clean build, which needs
  libclang: Xcode tools, `winget install LLVM.LLVM` or `libclang-dev`, per the root `AGENTS.md`.
- **Replacement**: the bindings for each target CI and releases build (`aarch64-apple-darwin`,
  `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`) committed
  under `libraw/src/bindings/`, picked by `cfg(target)` in `sys.rs`, with the layout assertions
  bindgen already emits; a script regenerates them when the LibRaw submodule moves. The
  [bindgen book](https://rust-lang.github.io/rust-bindgen/) names both build-time and
  committed bindings as common; a `-sys` crate over a vendored library pinned at a release tag
  is the committed case. A target without its file fails to compile, naming it.
- **Cost**: four files to regenerate on a LibRaw bump, on a machine with libclang. Gone: 8
  build crates, and the libclang step from every checkout and CI lane.

### `tracing-subscriber`: one subscriber of our own

- **Uses**: `darkroom::init_tracing`: `RUST_LOG` (default `info,rmcp=warn`), formatted lines on
  stderr, and `LogRing`'s layer of each thread's warnings and errors; its `tracing-log` feature
  bridges the `log` records of palantir and wgpu.
- **Replacement**: a `tracing::Subscriber` that keeps no spans (Darkroom opens none; rmcp's
  and ort's spans are taken and dropped), filters by `target=level` directives and a default
  level, writes a line to stderr and the ring, plus a `log::Log` that forwards to it. About
  200 lines with tests. `tracing` itself stays: rmcp and ort emit through it.
- **Cost and risk**: `RUST_LOG` keeps its `target=level` form; `EnvFilter`'s span and field
  directives go, which nothing here uses. With `bindgen` gone too, `regex` and its two crates
  leave the build: 17 crates in all.

### `nalgebra`: defer

- **Uses**: `lumos::math::lstsq` (QR, and SVD where QR is rank-deficient), the DLT homography
  (SVD of a 2n × 9 system), thin-plate splines and the SIP and background polynomial fits
  (dense solves), in 7 files.
- **Why defer**: QR by Householder reflections is short and well understood, but an SVD that
  holds nalgebra's accuracy on ill-conditioned systems is not small: one-sided Jacobi is the
  accurate choice, and it needs its own convergence tests. Every registration snapshot of
  lumos would move by bits. Revisit if lumos's linear algebra needs SIMD of its own anyway.

### `sysinfo`: the OS calls

- **Uses**: `common::platform`: the system's available memory, this process's resident memory,
  and on Unix whether a path's file system is memory-backed.
- **Replacement**: each platform file asks its OS: macOS `host_statistics64` and `task_info`
  through `libc`; Linux `/proc/meminfo`, `/proc/self/statm` and `/proc/self/mountinfo`; Windows
  `GlobalMemoryStatusEx` and `GetProcessMemoryInfo` through `windows`, already a dependency.
  About 150 lines; `common::platform`'s tests already hold each figure to be reported.
- **Cost**: three small OS bindings of our own instead of one crate's.

## Kept

- **`ort`** (22): ONNX Runtime for the ML nodes. Its build downloads the runtime (`ureq` and
  TLS); a prebuilt runtime would need one more install step on every machine.
- **`zip`** (7): the `.darkroomz` package.
- **`notify`** (5): the shared library's folder watch and lens's file watches, which every OS
  serves through its own API.
- **`blake3`**, **`ron`**, **`bitcode`**, **`rfd`**, **`libz-sys`**, **`memmap2`**, **`uuid`**,
  **`tokio`**: each does work we should not.
- **`arrayvec`**, **`hashbrown`**, **`image`**, **`tiff`**, **`futures-util`**,
  **`tokio-util`**: other dependencies build them anyway; dropping ours removes nothing.

## Elsewhere

- **fits-well's `bitvec`** (5: bitvec, funty, radium, tap, wyz): its bit columns
  (`ColumnData::Bit`, `BitColumn::row`, `as_bit`) hand out `BitVec<u8, Msb0>`; a bit type of its
  own changes its public API, which its `AGENTS.md` asks a go-ahead for.

## Order

1. `tracing-subscriber`, then `bindgen`: together 17 crates and the libclang step.
2. `sysinfo`.
3. `rmcp`, if the decision is to own the protocol.
4. `nalgebra`, deferred.
