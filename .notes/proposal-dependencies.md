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
| `nalgebra` | 8 / 9 / 9 | lumos's least squares, DLT and TPS: QR, SVD, dense solves | Defer |
| `sysinfo` | 2 / 1 / 3, and a heavy compile | free memory, this process's memory, a path's file system | Do: the OS calls in `common::platform` |

`strum`, `clap` and `rand` have landed (11 crates), then `bindgen` and `tracing-subscriber`
(17 crates on each target, and libclang from every build): the build compiles 279 third-party
crates on macOS, 342 on Linux and 281 on Windows. `sysinfo` takes about 2 more out on macOS; with
`rmcp`, about 29.

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

1. `sysinfo`.
2. `rmcp`, if the decision is to own the protocol.
3. `nalgebra`, deferred.
