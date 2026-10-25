# Plan: imaginarium becomes a workspace crate

imaginarium stops being a submodule and becomes a member crate, as lumos is.
The 2D primitives that two crates keep in copies move to `common`, and the
crate's names and errors follow the workspace's rules. This plan holds item
5.2 of `.notes/proposal-parallel-work.md` ("imaginarium a workspace member"),
which is checked when batch 1 lands. Facts and counts:
`docs/research/2026-10-25-imaginarium-merge.md`. When the plan lands, each
lasting decision moves into `AGENTS.md`, `imaginarium/AGENTS.md` or the doc
comments of the code it governs, and this file is deleted.

## Decisions

- **imaginarium is a member crate.** Its planar `From`/`TryFrom` impls start
  parallel work, and no clippy ban can name a trait impl. As a member, it calls
  `common::par` and `common::fs` itself. Refused: bans and wrappers as for
  fits-well, which leave those impls open. Source: the proposal's decision
  "imaginarium moves into the workspace".
- **Its history comes along.** `git subtree add` of its `main` keeps the 130
  commits for `git log` and `git blame`. Refused: a flat
  copy in one commit, which keeps the history only in the GitHub repository.
- **The 2D grid and its geometry live in `common`.** `Buffer2`, `Size2us`,
  `Vec2us` and `URect` know nothing of images, and lumos, lens and darkroom use
  them. Refused: a new crate for four types, which `common`'s leaf rule already
  admits; leaving them where they are, which makes lens and darkroom depend on
  lumos for a size.
- **`URect` becomes `Rect2us`.** One suffix for the three geometry types.
- **`Buffer2` holds a `Size2us`.** Its constructors and `reshape` take a size,
  and `size()` replaces lumos's `From<&Buffer2<T>> for Size2us`. `width()` and
  `height()` stay as accessors. Refused: a `width, height` pair, which a call
  site can transpose and the compiler cannot see (the doc of `Size2us`).
- **One CPU detection, two kernel designs.** `SimdTier` moves to `common`, and
  lumos's `Avx2Fma::detect` reads it there. Refused: one kernel framework.
  lumos's `simd::Isa` has `f32` and `f64` lanes only, and imaginarium's kernels
  pack `u8` and `u16` and shuffle bytes. A merge rewrites every imaginarium
  kernel with no measured gain.
- **One bench policy.** `Kind`, `Scale` and `fresh_copies` move to
  `common::bench`, behind `internals`, from two copies that differ only in
  doc comments. Each crate keeps its own scenes and id parts. Refused:
  palantir's fixture, which a standalone crate cannot share.
- **Reads and writes take a reader or a writer.** `Image::read_file` and
  `Image::save_file` are removed, and their bans go with them. Callers open
  through `common::fs`, as lens and lumos do today.
- **What only tests reach is behind `internals`.** `drawing` and `Color` have
  callers only in lumos tests and `lumos::internals::visual`.
- **No re-export of `glam`.** lens depends on `glam` and names `glam::Vec2`.
  `glam` is a workspace dependency already.
- **The GPU backend stays behind `wgpu`.** No workspace
  crate turns it on today, but lens's `Image` reserves a device variant for
  it. The CPU-only lint set and the bench build keep it built and linted.
  Refused: deletion, about 1 500 lines with their tests, which a device path
  would write again.
- **The interleaved image is `InterleavedImage`.** `imaginarium::Image` takes
  the new name. The name pairs with lumos's planar `LinearImage` and
  with lens's `Pixels::InterleavedCpu`, and it removes three aliases.
  Refused: a new name for `lens::Image`, which is what a wire carries and what
  the editor calls an image.
- **The one-channel word is `mono`.** `ChannelCount::Mono`,
  `ColorFormat::MONO_U8` and the rest, as lumos's `ChannelLayout::Mono`
  (`mono` 739 times in lumos, 0 in imaginarium). `luma` keeps its one meaning,
  the Rec. 709 weighted sum. Refused: `L`, the PIL mode name, which no other
  crate uses; `gray`/`grey`, which the crates spell both ways.
- **The resample op is `AffineWarp`.** `imaginarium::Transform` and
  `lumos::Transform` are both public. lumos calls the resample through a
  transform a warp (`WarpConfig::warp`), and lumos keeps `Transform` for the
  transform. lumos's crate-wide `Warp` already names a warp to run, so the
  narrower use takes the qualified name, as the Names rule says.
- **Errors are cases, each in its domain.** The crate-wide `Error`, with six
  `String` variants, splits: `CodecError` for decode and encode, `DescError`
  for a descriptor, `PlanesError` for a planar conversion, `GpuError` for the
  device. Each keeps the foreign error as its own type. `ImageDesc` is valid by
  construction, as lumos's `ImageDimensions` is: `new` panics, `checked`
  returns the `DescError`. So `Image::new_black` returns no `Result`.
  `FileFormat::from_path` returns an `Option`, as `from_extension` does.
  Refused: one `Error` of messages, which a caller cannot match on and whose
  name every crate has.
- **The root's lanes cover it.** The drive table adds the CPU-only lint set
  `{ all-features-but = ["imaginarium/wgpu"] }` and
  `bench = "--features imaginarium/wgpu"`. The root's profiles, `clippy.toml`
  and `.cargo/config.toml` replace the crate's own.

## Plan

Each batch ends with `cargo drive check` and `cargo drive test` green from the
root. Each item states its proof.

### 2. Geometry and the grid in `common`

- [ ] `Size2us`, `Vec2us` and `URect` (as `Rect2us`) move from
  `lumos/src/math/` to `common` with their tests, public at its root. Proof:
  the moved tests pass in `common`.
- [ ] `Buffer2` holds a `Size2us`: `new`, `new_default`, `new_filled`,
  `from_fn` and `reshape` take `impl Into<Size2us>`, and `size()` replaces
  `Size2us::from(&buffer)`. Every caller is rewritten. Proof: the `Buffer2`
  tests, with sizes that are not square, so a transposed size fails.
- [ ] Every `width, height` pair in imaginarium becomes a `Size2us`:
  `ImageDesc`, `Preview`, the drawing canvas, the bench scene (`sky`), the
  test helpers (`create_test_image`) and each fn that takes the pair. Proof:
  the tests of each.

### 3. One CPU detection, one bench policy

- [ ] `SimdTier` moves to `common`, with `dispatch` and `at_or_below` public
  for imaginarium. lumos's `Avx2Fma::detect` imports it from `common`.
  `common`'s crate doc names CPU features. Proof: its test passes in
  `common`.
- [ ] `common::bench` holds `Kind`, `Scale` and `fresh_copies` behind
  `internals`, which turns on an optional `criterion`. The fixtures of
  imaginarium and lumos keep only their own parts. The reason of the root's
  `criterion` bans names `common::bench::Scale`. Proof:
  `cargo drive bench --test` runs every case of both crates with the ids
  they had.

### 4. Names

- [ ] The `glam` re-export goes, and lens depends on `glam`.
- [ ] `Image` becomes `InterleavedImage`, and the aliases `CpuImage`,
  `RawImage` and `LensImage` go.
- [ ] `Transform` becomes `AffineWarp`, and `GpuTransformPipeline` becomes
  `GpuAffineWarpPipeline`.
- [ ] `L` becomes `Mono` in `ChannelCount`, in the `ColorFormat` constants and
  in `ImagePixels`; `grey` becomes `mono` in names.
- [ ] `GpuContext` becomes `GpuCtx`; `ALL_FORMATS` becomes `ColorFormat::ALL`;
  byte counts take `len` (`SampleType::byte_len`, `ColorFormat::byte_len`,
  `ImageDesc::byte_len`, `row_byte_len`); the rest of the research note's list
  follows. Then compare imaginarium's words in `cargo drive vocabulary` with
  the workspace's, and rename each word that has a second meaning. Proof: no
  word of the listing has two meanings.
- [ ] `drawing` and `Color` go behind `internals`, with `luma::WEIGHTS_F32`
  if only drawing reads it. Proof: the ship lane shows no dead code.

### 5. Errors

- [ ] `Error` splits into `CodecError`, `DescError`, `PlanesError` and
  `GpuError`. Each case holds fields, not a message, and a foreign error as
  its own type. `ImageDesc` has private fields, `new` and `checked`.
  `Image::new_black` returns the image. `FileFormat::from_path` returns an
  `Option`. lumos's `ImageError` and lens's `DecodeError::InvalidImage` take
  the new types. Proof: a test for each case that the code can return, with
  its fields; the descriptor tests take zero sides and an overflowing size.

## Changed

- Batch 1, the lanes: no `cross-lint`. The lane lints every feature, and
  `ort-sys` has no `x86_64-apple-darwin` binaries for lumos's `ml`, in
  rc.15 or on its `main` of 2026-10-23. `imaginarium/AGENTS.md` gives the
  one-crate clippy for the x86 kernels instead.
- Batch 1, the bench proof: `cargo drive bench --test` fails to link, as
  `.notes/ISSUES.md` records. imaginarium's own bench target ran instead: its
  391 ids equal those of `93cae6e`, and each case passed once.
- Batch 1, dependencies: `strum` and `tokio` stay, which the plan wrongly
  called unused (`BlendMode`'s derives, the GPU readback). `strum`'s `derive`
  replaces `strum_macros`. lumos names `glam`'s `f64`, which imaginarium's
  default features gave it before.

