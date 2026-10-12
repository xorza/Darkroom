# bench-proposal: open questions

## Item 4, `kernel/frame_store/spill/read/cold`: how a bench build evicts pages

A cold read needs the frame's pages out of the page cache before each pass. `MappedFile::evict`
does that on Linux with `msync`, `madvise(MADV_PAGEOUT)` and `sysconf`, all through `libc`. In
`lumos`, `libc` is a Linux dev-dependency only, so the eviction compiles under `cfg(test)` alone.
A criterion bench builds the library without `cfg(test)` and cannot reach it. memmap2 offers no
page-out advice, and `MADV_DONTNEED` drops only the mapping's pages, not the page cache, so it
does not evict.

| Option | What it does | Cost |
|---|---|---|
| **1. `libc` an optional Linux dependency, enabled by `bench` (recommended)** | `[target.'cfg(target_os = "linux")'.dependencies] libc = { workspace = true, optional = true }`, `bench = [..., "dep:libc"]`, and the eviction gated `any(test, feature = "bench")`. | The same crate and version as the dev-dependency; nothing new enters the tree. It is a change to `[dependencies]`, which needs your approval. |
| 2. No cold variants | Decision 4 is dropped: every spill and parking bench reads warm. | The disk read cost of a spilled run is not measured anywhere. |

Blocked: `kernel/frame_store/spill/read/cold` (item 4), and the cold variants of item 9
(`stage/park/warp_from_map/cold` and a spilled combine read cold).
