# Imaginarium

An image-processing library with **CPU (SIMD: SSE/AVX2/NEON) and GPU (wgpu compute) backends**, a unified CPU/GPU image buffer, and 9-format pixel polymorphism. A workspace member of Scenarium and a git submodule; consumed by `lens`/`lumos`. Pre-alpha, breaks freely.

`README.md` is the public-facing blurb; this file is the architecture map.

## Posture

- **No stability guarantees.** Rename, refactor, change signatures and rewrite callers. No compat shims.
- **GPU is optional, gated behind the `wgpu` feature** (`default = []`). Everything under `src/gpu/`, `GpuContext`, the `ops/*/gpu.rs` + `pipeline.rs` paths, and the `*Pipeline` / `Gpu*` re-exports in `lib.rs` only exist with `wgpu` on. CPU paths must compile and pass with the feature off.
- **SIMD correctness is cross-checked against scalar.** Every SIMD kernel has a scalar reference; tests compare the two. Don't add a SIMD path without the scalar fallback and the cross-check.
- **9 formats, always.** New ops and shaders handle the full `ALL_FORMATS` set (or explicitly declare a narrower supported list to `select_backend`). Half-covering the format matrix is a bug.
