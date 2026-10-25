# imaginarium in the workspace (2026-10-25)

Facts gathered for `PLAN.md`. Workspace at `4b4373674`, imaginarium at
`93cae6e` (its `origin/main`, 130 commits), cargo-drive 0.2.1. Host: macOS on
arm64, with the `x86_64-apple-darwin` target installed.

## The crate

- 80 Rust files, about 11 000 lines, with `src/`, `examples/` (5), `benches/`
  (one target) and `test_resources/` (one 1.4 MB TIFF).
- Its lint table is the workspace's, line for line. Its `clippy.toml` is a part
  of the root one: the root adds the I/O, thread and rayon bans. Its
  `.cargo/config.toml` is the root one, with a different comment.
- Its drive table: `lint = ["--all-features", ""]` (the second set is the CPU
  build), `bench = "--features wgpu"`, `docs = true`. The workspace table has
  none of these keys. It has no doc tests.
- `strum` and `strum_macros` give `BlendMode` its derives and iteration;
  `tokio` gives the GPU readback its `oneshot`. `strum` is on the
  `cargo-machete` ignore list.
- Its `glam` dependency took the default features, which unified the `f64`
  types into lumos's build: lumos names `DVec2` and `DMat2` and asked only
  for `serde` and `std`.
- `README.md` names `processing_context/`, `rust-toolchain.toml` and
  `#[ignore]` benches. None of them exist.
- `src/lib.rs` re-exports `glam::{Affine2, Vec2}`. `lens` takes `Vec2` from
  there (`lens/src/image/nodes/processing.rs:3`) and does not depend on `glam`.
- The module `src/common/` has the name of the `common` crate, which a member
  crate depends on.

## What the workspace takes from it

- Files that name it: lumos 176, lens 12, darkroom 5.
- `Buffer2` is 184 of the imported names. Then `ColorFormat` 14, `Image` 12,
  `Error` 10, `ImageDesc` 14, `FileFormat` 7, `SimdTier` 1, `drawing` 4,
  `Color` 3.
- No crate turns on the `wgpu` feature, so the GPU backend builds only in
  imaginarium's own lanes. `lens/src/image/mod.rs` states that nothing in
  `lens` opens a GPU.
- `drawing` and `Color` have callers only in lumos tests and in
  `lumos::internals::visual`.
- lumos takes `SimdTier` only in `Avx2Fma::detect`
  (`lumos/src/simd/avx2_fma.rs:27`).

## I/O and threads outside `common`

- `Image::read_file` and `Image::save_file` open files with `std::fs::File`.
  The root `clippy.toml` already refuses both methods to the workspace.
- `src/bench/fixture.rs` uses `std::{env, fs, process, thread}` directly.
- `examples/common/mod.rs` uses `std::fs` and `std::env`.
- The tests use `tempfile` and `std::fs::read`.

## Rayon outside `common::par`

Twelve calls start parallel work: `Buffer2::interleave` and
`Buffer2::deinterleave`; `image/conversion/mod.rs:35`;
`ops/contrast_brightness/cpu/mod.rs:126` and `:170`;
`ops/blend/cpu/mod.rs:84` and `:109`; `ops/transform/cpu/mod.rs:141`,
`sse.rs:222` and `neon.rs:183`; `ops/preview/cpu/mod.rs:111`;
`bench/scene.rs:37`. One more is in test code, `common/image_diff.rs:38`. The
planar `From` and `TryFrom` impls of `Image` (`image/mod.rs`, the
`planar_conversions!` macro) reach the two `Buffer2` calls.

## Duplicates

- lumos keeps `Size2us`, `Vec2us` and `URect` in `lumos/src/math/`, and
  `From<&Buffer2<T>> for Size2us`. Call sites: `Size2us::new` 776, `Vec2us`
  621, `URect` 76, `Size2us::from(&…)` 25. imaginarium passes `width, height`
  pairs: `Buffer2::new` 177 sites, `new_default` 140, `new_filled` 167,
  `from_fn` 91 in all crates.
- The bench policy is in two copies: `Kind`, `Scale` and `fresh_copies` in
  `imaginarium/src/bench/fixture.rs` and `lumos/src/bench/fixture.rs` differ
  only in doc comments. palantir and fits-well keep their own, as standalone
  crates.
- Two SIMD designs: imaginarium writes a kernel per tier (SSE2, SSSE3, SSE4.1,
  AVX2, NEON) over `u8`, `u16` and `f32` with byte shuffles; lumos writes one
  kernel over `simd::Isa`, whose vectors are `f32` and `f64` lanes only.

## Names

- `Image`: `imaginarium::Image` and `lens::Image`. Aliases that work around
  it: `CpuImage` (2), `RawImage` (2), `LensImage` (2).
- `Transform`: `imaginarium::Transform` (the affine resample op) and
  `lumos::Transform` (the registration transform), both public. lumos calls
  the resample through a transform a warp (`WarpConfig::warp`), and its
  `pub(crate) struct Warp` holds a warp to run: the transform and how it
  samples.
- `Error` and `Result` at the crate root. Six of the nine variants hold a
  `String` message. lumos has its own `ImageError`.
- The one-channel word: `mono` 739 times in lumos, 17 in darkroom, 0 in
  imaginarium; imaginarium says `L` (`ChannelCount::L`, `ColorFormat::L_U8`),
  `grey` (42) and `luminance`. lumos says `gray` (155).
- Against the fixed short forms and the `len` rule: `GpuContext`,
  `SampleType::size`, `ColorFormat::byte_count`, `ImageDesc::size_in_bytes`,
  `row_bytes`, `elem_size`, `img1`/`img2`, `channels: usize`, `ALL_FORMATS`
  beside `FileFormat::ALL` and `SimdTier::ALL`.
