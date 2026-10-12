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

## Item 10, `pipeline/drizzle/rggb`: a production defect blocks the default configuration

The bench drizzles twelve calibrated 6000 × 4000 RGGB mosaics from their FITS files with
`drizzle_cfa_stack` under the default configuration. Every run fails with
`NoNoiseToWeigh { index: 0 }`: at 6000 × 4000 the frame statistics give the blue colour no noise,
so the light preset's noise weighting has nothing to weigh by. The defect is in production code,
outside the bench plan, and is in `.notes/ISSUES.md`.

| Option | What it does | Cost |
|---|---|---|
| **1. Fix the defect, then add the bench at the defaults (recommended)** | The bench measures what a user's run does. | A change to the frame statistics, outside this plan. |
| 2. Bench with equal weighting until the fix | The bench runs now. | It measures a configuration a user does not start from, and hides the defect from the bench. |
| 3. Bench another frame size | The bench runs now. | Off the standard frame, which every stage and pipeline bench uses. |

Blocked: `pipeline/drizzle/rggb` (item 10). The bench writes the set as calibrated mosaics, a
dithered `Field` exposure through `Readout::Calibrated` saved by `CfaImage::save_fits`, and runs
`drizzle_cfa_stack` over them with each exposure's dither as its warp.
